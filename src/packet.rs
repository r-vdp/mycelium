use std::net::{IpAddr, Ipv6Addr};

use crate::{babel, crypto::PublicKey, metric::Metric, peer::Peer, sequence_number::SeqNo};

pub const BABEL_MAGIC: u8 = 42;
pub const BABEL_VERSION: u8 = 2;

/* ********************************PAKCET*********************************** */
#[derive(Debug, Clone)]
pub enum Packet {
    DataPacket(DataPacket),
    ControlPacket(ControlPacket),
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum PacketType {
    DataPacket = 0,
    ControlPacket = 1,
}

/* ******************************DATA PACKET********************************* */
#[derive(Debug, Clone)]
pub struct DataPacket {
    pub raw_data: Vec<u8>, // eccrypte data isself then append the nonce
    pub dest_ip: Ipv6Addr,
    pub pubkey: PublicKey,
}

impl DataPacket {}

/* ****************************CONTROL PACKET******************************** */

#[derive(Debug, Clone)]
pub struct ControlStruct {
    pub control_packet: ControlPacket,
    pub src_overlay_ip: IpAddr,
}

#[derive(Debug, PartialEq, Clone)]
pub struct ControlPacket {
    pub body: BabelPacketBody,
}

#[derive(Debug, PartialEq, Clone)]
pub struct BabelPacketHeader {
    pub magic: u8,
    pub version: u8,
    pub body_length: u16, // length of the whole BabelPacketBody (tlv_type, length and body)
}

// A BabelPacketBody describes exactly one TLV
#[derive(Debug, PartialEq, Clone)]
pub struct BabelPacketBody {
    pub tlv_type: BabelTLVType,
    pub length: u8, // length of the tlv (only the tlv, not tlv_type and length itself)
    pub tlv: babel::Tlv,
}

impl BabelPacketHeader {
    pub fn new(body_length: u16) -> Self {
        Self {
            magic: BABEL_MAGIC,
            version: BABEL_VERSION,
            body_length,
        }
    }
}

impl ControlPacket {
    pub fn new_hello(dest_peer: &mut Peer, interval: u16) -> Self {
        let tlv: babel::Tlv = babel::Hello::new_unicast(dest_peer.hello_seqno(), interval).into();
        dest_peer.increment_hello_seqno();
        Self {
            body: BabelPacketBody {
                tlv_type: BabelTLVType::Hello,
                length: tlv.wire_size(),
                tlv,
            },
        }
    }

    pub fn new_ihu(interval: u16, dest_address: IpAddr) -> Self {
        // TODO: Set rx metric
        let tlv: babel::Tlv = babel::Ihu::new(Metric::from(0), interval, Some(dest_address)).into();
        Self {
            body: BabelPacketBody {
                tlv_type: BabelTLVType::IHU,
                length: tlv.wire_size(),
                tlv,
            },
        }
    }

    pub fn new_update(
        plen: u8,
        interval: u16,
        seqno: SeqNo,
        metric: Metric,
        prefix: IpAddr,
        router_id: PublicKey,
    ) -> Self {
        let tlv: babel::Tlv =
            babel::Update::new(plen, 0, interval, seqno, metric, prefix, router_id).into();
        Self {
            body: BabelPacketBody {
                tlv_type: BabelTLVType::Update,
                length: tlv.wire_size(),
                tlv,
            },
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum BabelTLVType {
    Hello = 4,
    IHU = 5,
    Update = 8,
}

impl BabelTLVType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            4 => Some(Self::Hello),
            5 => Some(Self::IHU),
            8 => Some(Self::Update),
            _ => None,
        }
    }
}
