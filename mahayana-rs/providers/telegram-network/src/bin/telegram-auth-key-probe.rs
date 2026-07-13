use std::{env, process};

use fabushi_telegram_network::{establish_auth_key, NetworkConfig};
use fabushi_telegram_protocol::DcDirectory;

fn main() {
    let mut test_mode = false;
    let mut dc_id = 2_i32;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--test" => test_mode = true,
            "--dc" => {
                dc_id = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage("--dc requires a positive integer"));
            }
            "--help" | "-h" => usage(""),
            _ => usage(&format!("unknown argument: {argument}")),
        }
    }

    let directory = DcDirectory::telegram_defaults(test_mode, dc_id).unwrap_or_else(|error| {
        eprintln!("failed to build Telegram endpoint directory: {error}");
        process::exit(1);
    });
    let mut session = establish_auth_key(&directory, dc_id, &NetworkConfig::default())
        .unwrap_or_else(|error| {
            eprintln!("Telegram auth-key probe failed: {error}");
            process::exit(1);
        });
    let pong = session.ping().unwrap_or_else(|error| {
        eprintln!("Telegram encrypted ping failed: {error}");
        process::exit(1);
    });
    let mut future_salts_request = Vec::with_capacity(8);
    future_salts_request.extend_from_slice(&0xb921_bd04_u32.to_le_bytes());
    future_salts_request.extend_from_slice(&1_i32.to_le_bytes());
    let rpc = session
        .invoke_raw(&future_salts_request)
        .unwrap_or_else(|error| {
            eprintln!("Telegram encrypted RPC failed: {error}");
            process::exit(1);
        });
    let rpc_constructor = u32::from_le_bytes(
        rpc.body[..4]
            .try_into()
            .expect("RPC result has a constructor"),
    );
    if rpc_constructor != 0xae50_0895 {
        eprintln!("Telegram encrypted RPC returned unexpected constructor 0x{rpc_constructor:08x}");
        process::exit(1);
    }
    println!(
        "{{\"ok\":true,\"testMode\":{},\"dcId\":{},\"endpoint\":\"{}\",\"authKeyId\":\"{:016x}\",\"serverTime\":{},\"encryptedPingId\":\"{}\",\"pongMessageId\":\"{}\",\"rpcResultConstructor\":\"0x{:08x}\"}}",
        test_mode,
        dc_id,
        session.endpoint.address,
        session.auth_key.id(),
        session.server_time,
        pong.ping_id,
        pong.response_message_id,
        rpc_constructor
    );
}

fn usage(message: &str) -> ! {
    if !message.is_empty() {
        eprintln!("{message}");
    }
    eprintln!("usage: telegram-auth-key-probe [--test] [--dc <id>]");
    process::exit(if message.is_empty() { 0 } else { 2 });
}
