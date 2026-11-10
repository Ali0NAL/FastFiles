//! Ağdaki diğer FastFiles cihazlarını otomatik bulma.
//!
//! Her cihaz birkaç saniyede bir küçük bir UDP paketi yayınlar:
//! - Her ağ arayüzünden multicast (239.255.53.18:53318)
//! - Her arayüzün alt ağ broadcast adresine ve 255.255.255.255'e
//!
//! Wi-Fi ve ethernet aynı alt ağdaysa iki yöntemden en az biri her zaman ulaşır.
//! Ulaşmadığı durumlar için arayüzde "IP ile ekle" seçeneği vardır.

use crate::node::Node;
use crate::protocol::{DeviceInfo, DISCOVERY_PORT, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockRef, Socket, Type};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::sleep;

const MCAST: Ipv4Addr = Ipv4Addr::new(239, 255, 53, 18);
const APP_TAG: &str = "fastfiles";

#[derive(Serialize, Deserialize)]
struct Packet {
    app: String,
    v: u32,
    kind: String,
    device: DeviceInfo,
}

/// (arayüz IP'si, broadcast adresi)
fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() => Some((v4.ip, v4.broadcast)),
            _ => None,
        })
        .collect()
}

/// Gerçek yerel alt ağlar (ağ adresi, maske). /32 VPN arayüzleri (ör. WARP) hariç.
fn local_subnets() -> Vec<(u32, u32)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() && v4.netmask != Ipv4Addr::BROADCAST => {
                let mask = u32::from(v4.netmask);
                Some((u32::from(v4.ip) & mask, mask))
            }
            _ => None,
        })
        .collect()
}

/// Arayüzde gösterilecek yerel ağ IP'leri.
/// Önce gerçek LAN adresleri; /32 VPN (ör. Cloudflare WARP) ve link-local adresler hariç.
pub fn local_ips() -> Vec<String> {
    let mut ips: Vec<(u8, Ipv4Addr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4)
                if !v4.ip.is_loopback() && !v4.ip.is_link_local() && v4.netmask != Ipv4Addr::BROADCAST =>
            {
                let o = v4.ip.octets();
                let rank = if o[0] == 192 && o[1] == 168 { 0 } else if v4.ip.is_private() { 1 } else { 2 };
                Some((rank, v4.ip))
            }
            _ => None,
        })
        .collect();
    ips.sort();
    ips.dedup();
    ips.into_iter().map(|(_, ip)| ip.to_string()).collect()
}

fn packet(node: &Node, kind: &str) -> Vec<u8> {
    serde_json::to_vec(&Packet {
        app: APP_TAG.into(),
        v: PROTOCOL_VERSION,
        kind: kind.into(),
        device: node.device_info(),
    })
    .unwrap_or_default()
}

fn make_socket() -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // Aynı bilgisayarda birden fazla örnek (test) çalışabilsin.
    s.set_reuse_address(true)?;
    s.set_broadcast(true)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(1)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).into())?;
    s.set_nonblocking(true)?;
    UdpSocket::from_std(s.into())
}

fn join_groups(sock: &UdpSocket, joined: &mut HashSet<Ipv4Addr>) {
    let sref = SockRef::from(sock);
    for (ip, _) in interfaces() {
        if !joined.contains(&ip) && sref.join_multicast_v4(&MCAST, &ip).is_ok() {
            joined.insert(ip);
        }
    }
}

async fn announce(sock: &UdpSocket, node: &Node, kind: &str) {
    let pkt = packet(node, kind);
    for (ip, bcast) in interfaces() {
        if SockRef::from(sock).set_multicast_if_v4(&ip).is_ok() {
            let _ = sock.send_to(&pkt, (MCAST, DISCOVERY_PORT)).await;
        }
        if let Some(b) = bcast {
            let _ = sock.send_to(&pkt, (b, DISCOVERY_PORT)).await;
        }
    }
    let _ = sock.send_to(&pkt, (Ipv4Addr::BROADCAST, DISCOVERY_PORT)).await;
}

pub async fn run(node: Arc<Node>) {
    let sock = match make_socket() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("[fastfiles] keşif soketi açılamadı: {e}");
            return;
        }
    };
    let mut joined = HashSet::new();
    join_groups(&sock, &mut joined);
    node.set_local_subnets(local_subnets());

    // Dinleyici
    {
        let sock = sock.clone();
        let node = node.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let received = tokio::select! {
                    r = sock.recv_from(&mut buf) => r,
                    _ = node.shutdown.cancelled() => break,
                };
                match received {
                    Ok((n, from)) => {
                        let Ok(p) = serde_json::from_slice::<Packet>(&buf[..n]) else { continue };
                        if p.app != APP_TAG || p.device.id == node.identity.id {
                            continue;
                        }
                        #[cfg(debug_assertions)]
                        eprintln!("[udp] {} {} from {}", p.kind, p.device.name, from);
                        match p.kind.as_str() {
                            "bye" => node.remove_peer(&p.device.id),
                            _ => node.upsert_peer(p.device, from.ip(), None),
                        }
                    }
                    // Windows, UDP'de ICMP "port unreachable" sonrası hata döndürebilir; yoksay.
                    Err(_) => sleep(Duration::from_millis(50)).await,
                }
            }
        });
    }

    // Duyurucu
    let mut tick: u64 = 0;
    loop {
        announce(&sock, &node, "announce").await;
        node.prune_peers();
        node.check_update_inbox();
        node.check_for_updates();
        node.flush_outbox();
        node.flush_polls();
        node.flush_games();
        node.watch_tick();
        node.check_avatars();
        node.status_tick();
        if tick % 600 == 0 {
            node.prune_resume();
        }
        tick += 1;
        if tick % 4 == 0 {
            // Ağ değişikliklerinde (Wi-Fi'ya bağlanma vb.) yeni arayüzlere katıl.
            join_groups(&sock, &mut joined);
            node.set_local_subnets(local_subnets());
        }
        let delay = if tick < 4 { Duration::from_millis(600) } else { Duration::from_millis(2500) };
        tokio::select! {
            _ = sleep(delay) => {}
            _ = node.shutdown.cancelled() => break,
        }
    }
}

/// Uygulama kapanırken "gidiyorum" paketi gönderir (senkron).
pub fn send_bye(node: &Node) {
    let pkt = packet(node, "bye");
    let Ok(s) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else { return };
    let _ = s.set_broadcast(true);
    let _ = s.set_multicast_ttl_v4(1);
    let _ = s.set_multicast_loop_v4(true);
    if s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into()).is_err() {
        return;
    }
    let send = |addr: Ipv4Addr| {
        let _ = s.send_to(&pkt, &SocketAddr::from((addr, DISCOVERY_PORT)).into());
    };
    for (ip, bcast) in interfaces() {
        if s.set_multicast_if_v4(&ip).is_ok() {
            send(MCAST);
        }
        if let Some(b) = bcast {
            send(b);
        }
    }
    send(Ipv4Addr::BROADCAST);
}
