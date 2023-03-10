use core::ptr::NonNull;

use alloc::format;
use alloc::{string::String, vec};
use lose_net_stack::packets::tcp::TCPPacket;
use lose_net_stack::{results::Packet, IPv4, LoseStack, MacAddress, TcpFlags};
use opensbi_rt::{print, println, sbi};
// use virtio_drivers::{VirtIONet, VirtIOHeader, MmioTransport};
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};

use crate::virtio_impls::HalImpl;

pub fn init() {
    let mut net = VirtIONet::<HalImpl, MmioTransport>::new(unsafe {
        MmioTransport::new(NonNull::new(0x1000_8000 as *mut VirtIOHeader).unwrap())
            .expect("failed to create net driver")
    })
    .expect("failed to create net driver");

    let lose_stack = LoseStack::new(
        IPv4::new(10, 0, 2, 15),
        MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
    );

    loop {
        info!("waiting for data");
        let mut buf = vec![0u8; 1024];
        let len = net.recv(&mut buf).expect("can't receive data");

        info!("receive {len} bytes from net");
        hexdump(&buf[..len]);

        let packet = lose_stack.analysis(&buf[..len]);
        info!("packet: {:?}", packet);

        match packet {
            Packet::ARP(arp_packet) => {
                let reply_packet = arp_packet
                    .reply_packet(lose_stack.ip, lose_stack.mac)
                    .expect("can't build reply");
                net.send(&reply_packet.build_data())
                    .expect("can't send net data");
            }
            Packet::UDP(udp_packet) => {
                info!(
                    "{}:{}(MAC:{}) -> {}:{}(MAC:{})  len:{}",
                    udp_packet.source_ip,
                    udp_packet.source_port,
                    udp_packet.source_mac,
                    udp_packet.dest_ip,
                    udp_packet.dest_port,
                    udp_packet.dest_mac,
                    udp_packet.data_len
                );
                info!(
                    "data: {}",
                    String::from_utf8_lossy(udp_packet.data.as_ref())
                );
                hexdump(udp_packet.data.as_ref());

                if String::from_utf8_lossy(udp_packet.data.as_ref()) == "this is a ping!" {
                    let data = r"reply".as_bytes();
                    let udp_reply_packet = udp_packet.reply(data);
                    net.send(&udp_reply_packet.build_data())
                        .expect("can't send using net dev");
                    break;
                }
            }
            Packet::TCP(tcp_packet) => {
                if tcp_packet.flags == TcpFlags::S {
                    // receive a tcp connect packet
                    let mut reply_packet = tcp_packet.ack();
                    reply_packet.flags = TcpFlags::S | TcpFlags::A;
                    let reply_data = &reply_packet.build_data();
                    net.send(&reply_data).expect("can't send to net");
                } else if tcp_packet.flags.contains(TcpFlags::F) {
                    // tcp disconnected
                    let reply_packet = tcp_packet.ack();
                    net.send(&reply_packet.build_data())
                        .expect("can't send to net");

                    let mut end_packet = reply_packet.ack();
                    end_packet.flags |= TcpFlags::F;
                    net.send(&end_packet.build_data())
                        .expect("can't send to net");
                } else {
                    info!(
                        "{}:{}(MAC:{}) -> {}:{}(MAC:{})  len:{}",
                        tcp_packet.source_ip,
                        tcp_packet.source_port,
                        tcp_packet.source_mac,
                        tcp_packet.dest_ip,
                        tcp_packet.dest_port,
                        tcp_packet.dest_mac,
                        tcp_packet.data_len
                    );
                    info!(
                        "data: {}",
                        String::from_utf8_lossy(tcp_packet.data.as_ref())
                    );

                    hexdump(tcp_packet.data.as_ref());
                    if tcp_packet.flags.contains(TcpFlags::A) && tcp_packet.data_len == 0 {
                        continue;
                    }

                    // handle tcp data
                    receive_tcp(&mut net, &tcp_packet)
                }
            }
            _ => {}
        }
    }
    info!("net stack example test successed!");
}

#[no_mangle]
pub fn hexdump(data: &[u8]) {
    const PRELAND_WIDTH: usize = 70;
    println!("{:-^1$}", " hexdump ", PRELAND_WIDTH);
    for offset in (0..data.len()).step_by(16) {
        for i in 0..16 {
            if offset + i < data.len() {
                print!("{:02x} ", data[offset + i]);
            } else {
                print!("{:02} ", "");
            }
        }

        print!("{:>6}", ' ');

        for i in 0..16 {
            if offset + i < data.len() {
                let c = data[offset + i];
                if c >= 0x20 && c <= 0x7e {
                    print!("{}", c as char);
                } else {
                    print!(".");
                }
            } else {
                print!("{:02} ", "");
            }
        }

        println!("");
    }
    println!("{:-^1$}", " hexdump end ", PRELAND_WIDTH);
}

// handle packet when receive a tcp packet
pub fn receive_tcp(net: &mut VirtIONet<HalImpl, MmioTransport>, tcp_packet: &TCPPacket) {
    const CONTENT: &str = include_str!("../index.html");
    let header = format!(
        "\
HTTP/1.1 200 OK\r\n\
Content-Type: text/html\r\n\
Content-Length: {}\r\n\
Connecion: keep-alive\r\n\
\r\n\
{}",
        CONTENT.len(),
        CONTENT
    );

    // is it a get request?
    if tcp_packet.data_len > 10 && tcp_packet.data[..4] == [0x47,0x45,0x54, 0x20] {
        let mut index = 0;
        for i in 4..tcp_packet.data_len {
            if tcp_packet.data[i] == 0x20 {
                index = i;
                break;
            }
        }

        let url = String::from_utf8_lossy(&tcp_packet.data[4..index]);
        info!("request for {}", url);
        if url == "/close" {
            let reply_packet = tcp_packet.ack();
            net.send(&reply_packet.build_data()).expect("can't send reply packet");
            sbi::legacy::shutdown();
        }
        let reply_packet = tcp_packet.reply(header.as_bytes());
        net.send(&reply_packet.build_data()).expect("can't send to");
    } else {
        let reply_packet = tcp_packet.ack();
        net.send(&reply_packet.build_data()).expect("can't send reply packet");
    }
}
