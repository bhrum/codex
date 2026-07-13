use fabushi_telegram_protocol::{
    audit_schema, parse_schema_catalog, MTPROTO_API_BASELINE, TD_API_BASELINE,
    TD_API_PINNED_COMMIT, TELEGRAM_API_BASELINE,
};
use std::{env, fs, process};

fn main() {
    let arguments: Vec<_> = env::args().skip(1).collect();
    let (baseline, path) = match arguments.as_slice() {
        [path] => (TD_API_BASELINE, path.as_str()),
        [kind, path] => {
            let baseline = match kind.as_str() {
                "td" => TD_API_BASELINE,
                "telegram" => TELEGRAM_API_BASELINE,
                "mtproto" => MTPROTO_API_BASELINE,
                _ => {
                    eprintln!("unknown schema kind {kind}; use td, telegram, or mtproto");
                    process::exit(2);
                }
            };
            (baseline, path.as_str())
        }
        _ => {
            eprintln!("usage: td-schema-audit [td|telegram|mtproto] <path-to-schema.tl>");
            process::exit(2);
        }
    };
    if path.is_empty() {
        eprintln!("schema path must not be empty");
        process::exit(2);
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            process::exit(2);
        }
    };
    match audit_schema(&bytes, baseline) {
        Ok(audit) => {
            let catalog = parse_schema_catalog(&String::from_utf8_lossy(&bytes))
                .expect("audit already validated the schema catalog");
            println!(
                "Telegram schema verified: name={} commit={} sha256={} types={} functions={} explicit_constructor_ids={}",
                baseline.name,
                TD_API_PINNED_COMMIT,
                audit.sha256,
                audit.stats.types,
                audit.stats.functions,
                catalog.explicit_constructor_count()
            );
        }
        Err(error) => {
            eprintln!("TDLib schema audit failed: {error}");
            process::exit(1);
        }
    }
}
