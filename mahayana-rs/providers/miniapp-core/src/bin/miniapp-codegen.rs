use fabushi_miniapp_core::host_api_spec_json;
use fabushi_miniapp_core::HOST_API_VERSION;
use fabushi_miniapp_core::HOST_SDK_VERSION;
use fabushi_miniapp_core::SPEC_SCHEMA_VERSION;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("failed to resolve repository root")?;

    let spec_json = host_api_spec_json();

    write_dart(
        &repo_root.join("fabushi/lib/models/mini_app_host_spec_generated.dart"),
        &spec_json,
    )?;
    write_typescript(
        &repo_root.join("frontend/packages/miniapp-sdk/src/miniapp-host-spec.generated.ts"),
        &spec_json,
    )?;

    println!(
        "generated Fabushi mini app host spec {} / SDK {}",
        HOST_API_VERSION, HOST_SDK_VERSION
    );
    Ok(())
}

fn write_dart(path: &Path, spec_json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let content = format!(
        r#"// GENERATED CODE - DO NOT EDIT BY HAND.
// Source: native/miniapp-core. Regenerate with:
//   cargo run --manifest-path native/miniapp-core/Cargo.toml --bin miniapp-codegen

const int miniAppSpecSchemaVersion = {schema_version};
const String miniAppHostApiVersion = '{api_version}';
const String miniAppHostSdkVersion = '{sdk_version}';

const String miniAppHostSpecJson = r'''
{spec_json}
''';
"#,
        schema_version = SPEC_SCHEMA_VERSION,
        api_version = HOST_API_VERSION,
        sdk_version = HOST_SDK_VERSION,
        spec_json = spec_json,
    );
    fs::write(path, content)?;
    Ok(())
}

fn write_typescript(path: &Path, spec_json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let content = format!(
        r#"// GENERATED CODE - DO NOT EDIT BY HAND.
// Source: native/miniapp-core. Regenerate with:
//   cargo run --manifest-path native/miniapp-core/Cargo.toml --bin miniapp-codegen

export const MINIAPP_SPEC_SCHEMA_VERSION = {schema_version};
export const MINIAPP_HOST_API_VERSION = "{api_version}";
export const MINIAPP_HOST_SDK_VERSION = "{sdk_version}";

export const MINIAPP_HOST_SPEC = {spec_json} as const;

export const MINIAPP_HOST_CAPABILITIES = MINIAPP_HOST_SPEC.capabilities;
export const MINIAPP_HOST_NATIVE_CAPABILITIES = MINIAPP_HOST_SPEC.nativeCapabilities;
export const MINIAPP_HOST_METHODS = MINIAPP_HOST_SPEC.methods;
export type MiniAppHostMethod = typeof MINIAPP_HOST_METHODS[number]["method"];
export type MiniAppCapabilityId = typeof MINIAPP_HOST_CAPABILITIES[number]["id"];
"#,
        schema_version = SPEC_SCHEMA_VERSION,
        api_version = HOST_API_VERSION,
        sdk_version = HOST_SDK_VERSION,
        spec_json = spec_json,
    );
    fs::write(path, content)?;
    Ok(())
}
