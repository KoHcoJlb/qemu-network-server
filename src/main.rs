use std::{env, thread};
use std::any::Any;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use flume::Sender;
use pnet::datalink;
use pnet::datalink::{Channel, DataLinkReceiver};
use pnet::packet::ethernet::EthernetPacket;
use pnet::packet::Packet as PnetPacket;
use pnet::util::MacAddr;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;
use tun_tap::{Iface, Mode};

struct Peer {
    endpoint: SocketAddr,
    last_activity: Instant,
}

#[derive(Debug)]
enum Packet {
    Local(EthernetPacket<'static>),
    Remote(EthernetPacket<'static>, SocketAddr),
    Error(anyhow::Error),
}

fn interface_name() -> Result<String> {
    env::var("INTERFACE").or(Err(anyhow!("INTERFACE not specified")))
}

fn recv_local(tx: Sender<Packet>, iface: Arc<Iface>) {
    thread::spawn(move || {
        fn handle(iface: &Arc<Iface>) -> Result<EthernetPacket<'static>> {
            let mut buf = vec![0; 2000];
            let read = iface.recv(&mut buf).context("recv")?;
            let packet = EthernetPacket::owned(buf[..read].to_vec())
                .ok_or(anyhow!("malformed packet"))?;
            return Ok(packet);
        }

        loop {
            match handle(&iface) {
                Ok(packet) => tx.send(Packet::Local(packet)).unwrap(),
                Err(err) => tx.send(Packet::Error(err)).unwrap()
            }
        }
    });
}

fn recv_remove(tx: Sender<Packet>, mut socket: UdpSocket) {
    thread::spawn(move || {
        fn handle(socket: &mut UdpSocket) -> Result<(EthernetPacket<'static>, SocketAddr)> {
            let mut data = vec![0; 2000];
            let (read, addr) = socket.recv_from(&mut data).context("recv_from")?;

            let packet = EthernetPacket::owned(data[..read].to_vec())
                .ok_or(anyhow!("{addr}: malformed packet"))?;
            Ok((packet, addr))
        }

        loop {
            match handle(&mut socket) {
                Ok((packet, addr)) =>
                    tx.send(Packet::Remote(packet, addr)).unwrap(),
                Err(err) => tx.send(Packet::Error(err)).unwrap()
            }
        }
    });
}

fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_str("qemu_network_server=debug").unwrap())
        .init();

    let interfaces = datalink::interfaces();
    // dbg!(interfaces);

    let interface_name = interface_name()?;
    info!(interface_name);
    let iface = Arc::new(Iface::without_packet_info(&interface_name, Mode::Tap)
        .context("create tap interface")?);

    let socket = UdpSocket::bind("0.0.0.0:8889")
        .context("bind udp socket")?;

    let (tx, rx) = flume::bounded(0);
    recv_local(tx.clone(), iface.clone());
    recv_remove(tx.clone(), socket.try_clone().context("clone socket")?);

    let mut peers = HashMap::<MacAddr, Peer>::new();
    let mut last_timeout = Instant::now();
    loop {
        let packet = rx.recv().unwrap();

        if last_timeout.elapsed().as_secs() > 30 {
            last_timeout = Instant::now();
            peers.retain(|_, p| p.last_activity.elapsed().as_secs() < 60);
        }

        match packet {
            Packet::Remote(packet, addr) => {
                trace!(?packet, ?addr, "IN");

                if let Err(err) = iface.send(packet.packet()) {
                    warn!(?err, "send packet error");
                }

                if peers.insert(packet.get_source(), Peer {
                    endpoint: addr,
                    last_activity: Instant::now(),
                }).is_none() {
                    debug!(endpoint = ?addr, mac_addr = ?packet.get_source(), "new peer");
                    debug!(peers = ?peers.keys().collect::<Vec<_>>());
                }
            }
            Packet::Local(packet) => {
                trace!(?packet, "OUT");

                let peers: Vec<_> = if packet.get_destination().is_broadcast() {
                    peers.values().collect()
                } else {
                    peers.get(&packet.get_destination())
                        .into_iter()
                        .collect()
                };

                for p in peers {
                    if let Err(err) = socket.send_to(packet.packet(), p.endpoint) {
                        warn!(?err, "send packet error");
                    }
                }
            }
            Packet::Error(err) => {
                error!(?err, "receive error")
            }
        }
    }
}
