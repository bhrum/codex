use super::*;

struct TestWorkspace(PathBuf);

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("mahayana-cli-{name}-{}", now_millis()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write_app(&self, html: &str) {
        fs::write(self.0.join("index.html"), html).unwrap();
        write_json_file(
            &self.0.join("manifest.json"),
            &json!({
                "schemaVersion": 1,
                "miniAppId": "local.test",
                "title": "测试小程序",
                "version": "0.0.1",
                "permissions": ["app.context"],
                "entry": "index.html",
            }),
        )
        .unwrap();
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn extracts_only_complete_html_documents() {
    let source = "说明\n```html\n<!DOCTYPE html><html><body>南无阿弥陀佛</body></html>\n```";
    assert_eq!(
        extract_html(source),
        Some("<!DOCTYPE html><html><body>南无阿弥陀佛</body></html>")
    );
    assert_eq!(extract_html("<html><body>截断"), None);
}

#[test]
fn inspection_accepts_safe_apps_and_rejects_remote_code() {
    let safe = TestWorkspace::new("safe");
    safe.write_app("<!DOCTYPE html><html><body>安全</body></html>");
    let safe_result = inspect_miniapp(&safe.0).unwrap();
    assert_eq!(safe_result["passed"], true);
    assert_eq!(safe_result["errors"], json!([]));

    let unsafe_app = TestWorkspace::new("unsafe");
    unsafe_app.write_app(
        "<!DOCTYPE html><html><body><script src='https://example.test/app.js'></script></body></html>",
    );
    let unsafe_result = inspect_miniapp(&unsafe_app.0).unwrap();
    assert_eq!(unsafe_result["passed"], false);
    assert_eq!(unsafe_result["errors"], json!(["禁止加载外部脚本"]));
}
