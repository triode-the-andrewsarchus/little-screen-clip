use super::*;
use std::io::Cursor;

static STOP: AtomicBool = AtomicBool::new(false);

#[test]
fn tcp_declared_payload_is_known_without_a_captured_payload() {
    let mut ip = [0u8; 40];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&85_u16.to_be_bytes());
    ip[9] = 6;
    ip[20..].copy_from_slice(&tcp_header());
    let fields = protocol::inspect(228, &ip);
    assert_eq!(
        fields.tcp,
        Some(TcpSegment {
            sequence: 0xffff_fffe,
            acknowledgment: 12,
            flags: 0x18,
            advertised_window: 4096,
            payload_len: Some(41),
        })
    );
    assert!(protocol::inspect(228, &ip[..39]).tcp.is_none());
    ip[6] = 0x20; // More fragments means this IP length is only a fragment.
    assert_eq!(protocol::inspect(228, &ip).tcp.unwrap().payload_len, None);
}

#[test]
fn tcp_payload_length_excludes_ipv6_extensions_and_rejects_unknown_extents() {
    let mut ip = [0u8; 68];
    ip[0] = 0x60;
    ip[4..6].copy_from_slice(&112_u16.to_be_bytes());
    ip[6] = 60; // Destination options, eight bytes.
    ip[40] = 6;
    ip[48..].copy_from_slice(&tcp_header());
    assert_eq!(
        protocol::inspect(229, &ip).tcp.unwrap().payload_len,
        Some(80)
    );
    ip[4..6].fill(0); // A possible jumbogram needs separate length metadata.
    assert_eq!(protocol::inspect(229, &ip).tcp.unwrap().payload_len, None);
}

fn tcp_header() -> [u8; 20] {
    let mut tcp = [0; 20];
    tcp[4..8].copy_from_slice(&0xffff_fffe_u32.to_be_bytes());
    tcp[8..12].copy_from_slice(&12_u32.to_be_bytes());
    tcp[12] = 0x60; // 24-byte header including options.
    tcp[13] = 0x18;
    tcp[14..16].copy_from_slice(&4096_u16.to_be_bytes());
    tcp
}

fn eth_udp() -> Vec<u8> {
    let mut eth = vec![
        0x02, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 2, 0x81, 0, 0, 7, 0x08, 0,
    ];
    eth.extend([
        0x45, 0, 0, 32, 0, 1, 0, 0, 64, 17, 0, 0, 192, 0, 2, 4, 198, 51, 100, 8, 0x30, 0x39, 0x1f,
        0x90, 0, 12, 0, 0, b't', b'e', b's', b't',
    ]);
    eth
}

fn pcap(rows: u32, nano: bool, big: bool) -> Vec<u8> {
    let magic = if nano { 0xa1b23c4d } else { 0xa1b2c3d4 };
    let mut out = word32(magic, big).to_vec();
    for value in [2_u16, 4] {
        out.extend(word16(value, big));
    }
    for field in [0, 0, 65535, 1 | 1 << 26 | 2 << 28] {
        out.extend(word32(field, big));
    }
    let payload = eth_udp();
    let len = payload.len() as u32;
    for row in 0..rows {
        for field in [111_000_000 + row, 456_789, len, len + 4] {
            out.extend(word32(field, big));
        }
        out.extend(&payload);
    }
    out
}

fn word16(n: u16, big: bool) -> [u8; 2] {
    if big {
        n.to_be_bytes()
    } else {
        n.to_le_bytes()
    }
}

fn word32(n: u32, big: bool) -> [u8; 4] {
    if big {
        n.to_be_bytes()
    } else {
        n.to_le_bytes()
    }
}

fn ng_block(out: &mut Vec<u8>, big: bool, kind: u32, body: &[u8]) {
    let padded = body.len().next_multiple_of(4);
    let size = word32(padded as u32 + 12, big);
    out.extend(word32(kind, big));
    out.extend(size);
    out.extend(body);
    out.extend(&[0; 3][..padded - body.len()]);
    out.extend(size);
}

fn ng_section(out: &mut Vec<u8>, big: bool) {
    let mut body = word32(0x1a2b3c4d, big).to_vec();
    body.extend(word16(1, big));
    body.extend([0; 2]);
    body.extend([255; 8]);
    ng_block(out, big, 0x0a0d0d0a, &body);
}

fn ng_interface(out: &mut Vec<u8>, big: bool, link: u16, resolution: u8, offset: i64) {
    let mut body = word16(link, big).to_vec();
    body.extend([0; 2]);
    body.extend(word32(65535, big));
    body.extend(word16(9, big));
    body.extend(word16(1, big));
    body.extend([resolution, 0, 0, 0]);
    body.extend(word16(14, big));
    body.extend(word16(8, big));
    body.extend(if big {
        offset.to_be_bytes()
    } else {
        offset.to_le_bytes()
    });
    body.extend([0; 4]);
    ng_block(out, big, 1, &body);
}

fn ng_packet(out: &mut Vec<u8>, big: bool, ordinal: u32, ticks: u64, payload: &[u8]) {
    let mut body = Vec::new();
    for value in [
        ordinal,
        (ticks >> 32) as u32,
        ticks as u32,
        payload.len() as u32,
        payload.len() as u32,
    ] {
        body.extend(word32(value, big));
    }
    body.extend(payload);
    ng_block(out, big, 6, &body);
}

#[test]
fn classic_network_fields_and_precision_survive_exact_window_export() {
    for big in [false, true] {
        let source = pcap(725, big, big);
        let mut reader = Reader::open(Cursor::new(&source)).unwrap();
        let index = reader.build_index(&STOP).unwrap();
        assert_eq!(index.points(), 725);
        assert_eq!(index.interfaces().next().unwrap().fcs_bits, Some(32));
        let packet = reader.read_packet(&index, 400, &STOP).unwrap();
        let protocol = packet.protocol();
        assert_eq!(protocol.protocol, Protocol::Udp);
        assert_eq!(protocol.source_port, Some(12345));
        assert_eq!(protocol.destination_port, Some(8080));
        assert_eq!(protocol.vlan, Some(7));
        assert_eq!(protocol.source.unwrap().to_string(), "192.0.2.4");
        let mut bytes = Vec::new();
        assert_eq!(
            reader.export(&index, 400..402, &mut bytes, &STOP).unwrap(),
            2
        );
        let mut output = Reader::open(Cursor::new(bytes)).unwrap();
        let first = output.next_packet(&STOP).unwrap().unwrap();
        assert_eq!(first.header.time, packet.header.time);
        assert_eq!(first.header.original_len, packet.header.original_len);
        assert_eq!(first.bytes, packet.bytes);
    }
}

#[test]
fn pcapng_sections_retain_interface_time_and_untimed_records_after_seeking_and_export() {
    let mut source = Vec::new();
    let payload = eth_udp();
    ng_section(&mut source, false);
    ng_interface(&mut source, false, 1, 9, 20);
    for row in 0..260 {
        ng_packet(&mut source, false, 0, 30_000_000 + row, &payload);
    }
    ng_interface(&mut source, false, 101, 6, 20);
    ng_packet(&mut source, false, 1, 42_000_000, &payload[18..]);
    ng_section(&mut source, true);
    ng_interface(&mut source, true, 1, 0x80 | 10, -12);
    let mut simple = word32(payload.len() as u32, true).to_vec();
    simple.extend(&payload);
    ng_block(&mut source, true, 3, &simple);
    ng_packet(&mut source, true, 0, 10_752, &payload);

    let mut reader = Reader::open(Cursor::new(&source)).unwrap();
    let index = reader.build_index(&STOP).unwrap();
    assert_eq!(index.points(), 263);
    assert_eq!(index.interfaces().count(), 3);
    assert_eq!(index.untimed_packets(), 1);
    let window = reader.read_window(&index, 258..263, &STOP).unwrap();
    assert_eq!(
        window[2].header.interface,
        InterfaceId {
            section: 0,
            ordinal: 1
        }
    );
    assert_eq!(
        window[3].header.interface,
        InterfaceId {
            section: 1,
            ordinal: 0
        }
    );
    assert_eq!(window[3].header.time, None);
    assert_eq!(
        window[4].header.time.unwrap().unix_nanoseconds(),
        -1_500_000_000
    );
    assert_eq!(
        index.time_bounds_ns(),
        Some([-1_500_000_000, 62_000_000_000])
    );
    assert_eq!(
        index.activity().iter().map(|bin| bin.packets).sum::<u64>(),
        262
    );

    let mut bytes = Vec::new();
    assert_eq!(
        reader.export(&index, 260..263, &mut bytes, &STOP).unwrap(),
        3
    );
    let mut output = Reader::open(Cursor::new(bytes)).unwrap();
    let output_index = output.build_index(&STOP).unwrap();
    let output_window = output.read_window(&output_index, 0..3, &STOP).unwrap();
    for (row, expected) in output_window.iter().zip(&window[2..]) {
        assert_eq!(row.header.time, expected.header.time);
        assert_eq!(row.header.interface, expected.header.interface);
        assert_eq!(row.header.link_type, expected.header.link_type);
    }
}

#[test]
fn prefix_and_compressed_random_windows_do_not_require_unbounded_storage() {
    let source = pcap(10_000, true, false);
    let preview = summary(Cursor::new(&source), &STOP).unwrap();
    assert_eq!(preview.packets.len(), PREFIX_ROWS);
    assert!(!preview.complete);

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&source).unwrap();
    let gzip = encoder.finish().unwrap();
    let zstd = zstd::encode_all(&source[..], 1).unwrap();
    let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
    encoder.write_all(&source).unwrap();
    let lz4 = encoder.finish().unwrap();
    for (compressed, mode, extension) in [
        (gzip, Compression::Gzip, "recording.pcap.gz"),
        (zstd, Compression::Zstd, "recording.pcap.zst"),
        (lz4, Compression::Lz4, "recording.pcap.lz4"),
    ] {
        assert!(recognizes(&compressed, Path::new(extension)));
        let mut reader = Reader::open(Cursor::new(compressed)).unwrap();
        assert_eq!(reader.format().compression, mode);
        let index = reader.build_index(&STOP).unwrap();
        assert_eq!(index.points(), 10_000);
        assert_eq!(
            reader.read_packet(&index, 9721, &STOP).unwrap().bytes,
            eth_udp()
        );
        assert_eq!(
            reader.read_window(&index, 123..126, &STOP).unwrap()[2]
                .header
                .row,
            125
        );
    }
}

#[test]
fn truncated_final_payload_is_rejected_in_index_and_cap_needs_a_real_header() {
    let mut source = pcap(1, false, false);
    assert!(recognizes(&source, Path::new("confirmed.cap")));
    assert!(!recognizes(b"unrelated capture", Path::new("unknown.cap")));
    source.pop();
    let mut reader = Reader::open(Cursor::new(source)).unwrap();
    assert!(matches!(reader.build_index(&STOP), Err(Error::Truncated)));
}
