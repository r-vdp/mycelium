use std::{net::IpAddr, net::SocketAddr, str::FromStr, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, error};

use mycelium::{
    crypto::PublicKey,
    endpoint::Endpoint,
    metrics::Metrics,
    peer_manager::{PeerExists, PeerNotFound, PeerStats},
};

#[cfg(feature = "message")]
mod message;
#[cfg(feature = "message")]
pub use message::{MessageDestination, MessageReceiveInfo, MessageSendInfo, PushMessageResponse};

/// Http API server handle. The server is spawned in a background task. If this handle is dropped,
/// the server is terminated.
pub struct Http {
    /// Channel to send cancellation to the http api server. We just keep a reference to it since
    /// dropping it will also cancel the receiver and thus the server.
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone)]
/// Shared state accessible in HTTP endpoint handlers.
struct HttpServerState<M> {
    /// Access to the (`node`)(mycelium::Node) state.
    node: Arc<Mutex<mycelium::Node<M>>>,
}

impl Http {
    /// Spawns a new HTTP API server on the provided listening address.
    pub fn spawn<M>(node: mycelium::Node<M>, listen_addr: SocketAddr) -> Self
    where
        M: Metrics + Clone + Send + Sync + 'static,
    {
        let server_state = HttpServerState {
            node: Arc::new(Mutex::new(node)),
        };
        let admin_routes = Router::new()
            .route("/admin", get(get_info))
            .route("/admin/peers", get(get_peers).post(add_peer))
            .route("/admin/peers/:endpoint", delete(delete_peer))
            .route("/admin/routes/selected", get(get_selected_routes))
            .route("/admin/routes/fallback", get(get_fallback_routes))
            .route("/pubkey/:ip", get(get_pubk_from_ip))
            .with_state(server_state.clone());
        let app = Router::new().nest("/api/v1", admin_routes);
        #[cfg(feature = "message")]
        let app = app.nest("/api/v1", message::message_router_v1(server_state));

        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(listen_addr).await {
                Ok(listener) => listener,
                Err(e) => {
                    error!("Failed to bind listener for Http Api server: {e}");
                    error!("API disabled");
                    return;
                }
            };

            let server =
                axum::serve(listener, app.into_make_service()).with_graceful_shutdown(async {
                    cancel_rx.await.ok();
                });

            if let Err(e) = server.await {
                error!("Http API server error: {e}");
            }
        });
        Http { _cancel_tx }
    }
}

/// Get the stats of the current known peers
async fn get_peers<M>(State(state): State<HttpServerState<M>>) -> Json<Vec<PeerStats>>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    debug!("Fetching peer stats");
    Json(state.node.lock().await.peer_info())
}

/// Payload of an add_peer request
#[derive(Deserialize, Serialize)]
pub struct AddPeer {
    /// The endpoint used to connect to the peer
    pub endpoint: String,
}

/// Add a new peer to the system
async fn add_peer<M>(
    State(state): State<HttpServerState<M>>,
    Json(payload): Json<AddPeer>,
) -> Result<StatusCode, (StatusCode, String)>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    debug!("Attempting to add peer {} to  the system", payload.endpoint);
    let endpoint = match Endpoint::from_str(&payload.endpoint) {
        Ok(endpoint) => endpoint,
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };

    match state.node.lock().await.add_peer(endpoint) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(PeerExists) => Err((
            StatusCode::CONFLICT,
            "A peer identified by that endpoint already exists".to_string(),
        )),
    }
}

/// remove an existing peer from the system
async fn delete_peer<M>(
    State(state): State<HttpServerState<M>>,
    Path(endpoint): Path<String>,
) -> Result<StatusCode, (StatusCode, String)>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    debug!("Attempting to remove peer {} to  the system", endpoint);
    let endpoint = match Endpoint::from_str(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };

    match state.node.lock().await.remove_peer(endpoint) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(PeerNotFound) => Err((
            StatusCode::NOT_FOUND,
            "A peer identified by that endpoint does not exist".to_string(),
        )),
    }
}

/// Alias to a [`Metric`](crate::metric::Metric) for serialization in the API.
pub enum Metric {
    /// Finite metric
    Value(u16),
    /// Infinite metric
    Infinite,
}

/// Info about a route. This uses base types only to avoid having to introduce too many Serialize
/// bounds in the core types.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// We convert the [`subnet`](Subnet) to a string to avoid introducing a bound on the actual
    /// type.
    pub subnet: String,
    /// Next hop of the route, in the underlay.
    pub next_hop: String,
    /// Computed metric of the route.
    pub metric: Metric,
    /// Sequence number of the route.
    pub seqno: u16,
}

/// List all currently selected routes.
async fn get_selected_routes<M>(State(state): State<HttpServerState<M>>) -> Json<Vec<Route>>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    debug!("Loading selected routes");
    let routes = state
        .node
        .lock()
        .await
        .selected_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// List all active fallback routes.
async fn get_fallback_routes<M>(State(state): State<HttpServerState<M>>) -> Json<Vec<Route>>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    debug!("Loading fallback routes");
    let routes = state
        .node
        .lock()
        .await
        .fallback_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// General info about a node.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// The overlay subnet in use by the node.
    pub node_subnet: String,
}

/// Get general info about the node.
async fn get_info<M>(State(state): State<HttpServerState<M>>) -> Json<Info>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    Json(Info {
        node_subnet: state.node.lock().await.info().node_subnet.to_string(),
    })
}

/// Public key from a node.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PubKey {
    /// The public key from the node
    pub public_key: PublicKey,
}

/// Get public key from IP.
async fn get_pubk_from_ip<M>(
    State(state): State<HttpServerState<M>>,
    Path(ip): Path<IpAddr>,
) -> Result<Json<PubKey>, StatusCode>
where
    M: Metrics + Clone + Send + Sync + 'static,
{
    match state.node.lock().await.get_pubkey_from_ip(ip) {
        Some(pubkey) => Ok(Json(PubKey { public_key: pubkey })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

impl Serialize for Metric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Infinite => serializer.serialize_str("infinite"),
            Self::Value(v) => serializer.serialize_u16(*v),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finite_metric_serialization() {
        let metric = super::Metric::Value(10);
        let s = serde_json::to_string(&metric).expect("can encode finite metric");

        assert_eq!("10", s);
    }

    #[test]
    fn infinite_metric_serialization() {
        let metric = super::Metric::Infinite;
        let s = serde_json::to_string(&metric).expect("can encode infinite metric");

        assert_eq!("\"infinite\"", s);
    }
}
