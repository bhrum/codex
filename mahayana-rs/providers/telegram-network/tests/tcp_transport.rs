use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use fabushi_telegram_network::{establish_auth_key, NetworkConfig, TcpTransport};
use fabushi_telegram_protocol::{
    DcDirectory, PlaintextEnvelope, TransportFrameCodec, TransportMode,
};

#[test]
fn abridged_transport_handles_fragmented_tcp_reads_and_writes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut header = [0_u8; 1];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header, [0xef]);

        let mut length = [0_u8; 1];
        stream.read_exact(&mut length).unwrap();
        let body_length = usize::from(length[0]) * 4;
        let mut request = vec![0_u8; body_length];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(request, vec![0x41; 12]);

        let mut codec = TransportFrameCodec::new(TransportMode::Abridged);
        let response = codec.encode(&[0x42; 16]).unwrap();
        for fragment in response.chunks(3) {
            stream.write_all(fragment).unwrap();
            thread::sleep(Duration::from_millis(2));
        }
    });

    let stream = TcpStream::connect(address).unwrap();
    let mut transport = TcpTransport::from_stream(stream, TransportMode::Abridged);
    transport.send_payload(&[0x41; 12]).unwrap();
    assert_eq!(transport.receive_payload().unwrap(), vec![0x42; 16]);
    server.join().unwrap();
}

#[test]
fn plaintext_exchange_preserves_wire_envelope() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut transport = TcpTransport::from_stream(stream, TransportMode::Intermediate);
        let payload = transport.receive_payload().unwrap();
        let request = PlaintextEnvelope::decode(&payload).unwrap();
        assert_eq!(request.body, vec![1, 2, 3, 4]);
        transport
            .send_payload(
                &PlaintextEnvelope {
                    message_id: 9,
                    body: vec![5, 6, 7, 8],
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
    });

    let stream = TcpStream::connect(address).unwrap();
    let mut transport = TcpTransport::from_stream(stream, TransportMode::Intermediate);
    let response = transport
        .exchange_plaintext(&PlaintextEnvelope {
            message_id: 4,
            body: vec![1, 2, 3, 4],
        })
        .unwrap();
    assert_eq!(response.message_id, 9);
    assert_eq!(response.body, vec![5, 6, 7, 8]);
    server.join().unwrap();
}

#[test]
#[ignore = "requires a live Telegram production data center"]
fn live_session_exchanges_encrypted_ping_and_service_rpc() {
    let directory = DcDirectory::telegram_defaults(false, 2).unwrap();
    let mut session = establish_auth_key(&directory, 2, &NetworkConfig::default()).unwrap();
    session.ping().unwrap();

    let mut request = Vec::with_capacity(8);
    request.extend_from_slice(&0xb921_bd04_u32.to_le_bytes());
    request.extend_from_slice(&1_i32.to_le_bytes());
    let result = session.invoke_raw(&request).unwrap();
    assert_eq!(
        u32::from_le_bytes(result.body[..4].try_into().unwrap()),
        0xae50_0895
    );
}
