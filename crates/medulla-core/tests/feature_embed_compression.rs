//! Embedded tool-output compression reaches the next model request.

#![recursion_limit = "512"]

use medulla::daemon::providers::{run_local_task, Abort, EmbedHost, RunTaskOptions, RunTaskOrigin};
use medulla::protocol::{HarnessProvider, HarnessTransport};
use serde_json::{json, Value};
use std::collections::HashMap;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const FIXTURE: &str = include_str!("fixtures/tool-output-long.json");

#[derive(Clone)]
struct ToolThenAnswer;

impl Respond for ToolThenAnswer {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let done = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool");
        let message = if done {
            json!({"role":"assistant", "content":"done"})
        } else {
            json!({"role":"assistant", "content":null,"tool_calls":[{"id":"call_fixture","type":"function","function":{
                "name":"shell", "arguments":json!({"command":fixture_command()}).to_string()}}]})
        };
        ResponseTemplate::new(200).set_body_json(json!({"id":"fixture", "object":"chat.completion", "created":1700000000,
            "model":"fixture", "choices":[{"index":0,"message":message,"finish_reason":if done {"stop"} else {"tool_calls"}}],
            "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}))
    }
}

#[cfg(unix)]
fn fixture_command() -> String {
    format!("printf '%s\\n' \"$(cat '{}')\"", fixture_path().display())
}
#[cfg(windows)]
fn fixture_command() -> String {
    format!("Get-Content -Raw '{}'", fixture_path().display())
}
fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool-output-long.json")
}

fn options(
    host: std::sync::Arc<EmbedHost>,
    home: &std::path::Path,
    cwd: &std::path::Path,
    uri: &str,
) -> RunTaskOptions {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("MEDULLA_HOME".into(), home.to_string_lossy().into_owned());
    env.insert("LOCAL_ROUTER_KEY".into(), "fixture-key".into());
    RunTaskOptions {
        task_id: None,
        embed: host,
        budget: None,
        provider: HarnessProvider::Openhuman,
        origin: RunTaskOrigin::Workflow,
        transport: HarnessTransport::Cli,
        prompt: "Read the JSON fixture.".into(),
        cwd: cwd.to_string_lossy().into_owned(),
        env,
        timeout_ms: 15_000,
        model: Some("fixture".into()),
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        conversation: String::new(),
        session_class: medulla::sessions::SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        abort: Abort::new(),
        router: Some(medulla::config::RouterConfig {
            base_url: Some(format!("{uri}/v1")),
            api_key_env: Some("LOCAL_ROUTER_KEY".into()),
            ..Default::default()
        }),
        attribution: false,
        hooks: Default::default(),
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

fn tool_contents(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if map.get("role").and_then(Value::as_str) == Some("tool") {
                if let Some(content) = map.get("content").and_then(Value::as_str) {
                    out.push(content.to_owned());
                }
            }
            for child in map.values() {
                tool_contents(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                tool_contents(child, out);
            }
        }
        _ => {}
    }
}

async fn run_fixture(
    host: std::sync::Arc<EmbedHost>,
) -> (Vec<Request>, tempfile::TempDir, tempfile::TempDir) {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(ToolThenAnswer)
        .mount(&server)
        .await;
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let result = run_local_task(options(host, home.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    assert_eq!(result.reply, "done");
    let requests = server.received_requests().await.unwrap();
    (requests, home, cwd)
}

#[test]
fn explicit_opt_out_preserves_exact_tool_output_with_tinyjuice_feature_enabled() {
    std::thread::Builder::new()
        .stack_size(medulla::tokio_tuning::WORKER_STACK_BYTES)
        .spawn(opt_out_scenario)
        .unwrap()
        .join()
        .unwrap();
}

fn opt_out_scenario() {
    medulla::tokio_tuning::build_runtime()
        .unwrap()
        .block_on(async {
            let (requests, home, _cwd) = run_fixture(std::sync::Arc::new(
                EmbedHost::default().with_compression_enabled(false),
            ))
            .await;
            let completions: Vec<_> = requests
                .iter()
                .filter(|request| {
                    request.method == wiremock::http::Method::POST
                        && request.url.path() == "/v1/chat/completions"
                })
                .collect();
            assert!(
                completions.len() >= 2,
                "expected a second provider completion request"
            );
            let second: Value = serde_json::from_slice(&completions[1].body).unwrap();
            let mut contents = Vec::new();
            tool_contents(&second, &mut contents);
            assert!(
                contents.iter().any(|content| content == FIXTURE),
                "tool content did not preserve fixture exactly; contents={:?}",
                contents
                    .iter()
                    .map(|c| (c.len(), c.chars().take(100).collect::<String>()))
                    .collect::<Vec<_>>()
            );
            let workspace = medulla::home::medulla_home(&HashMap::from([(
                "MEDULLA_HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]))
            .join("embed/workspace");
            assert!(medulla::juice::read_embedded_stats(&workspace)
                .unwrap()
                .is_none());
        });
}

#[test]
#[ignore = "requires an explicitly built TinyJuice cdylib via TINYJUICE_TEST_MODULE"]
fn loaded_tinyjuice_module_compresses_the_second_request_and_records_workspace_totals() {
    assert!(
        std::env::var_os("TINYJUICE_TEST_MODULE").is_some(),
        "set TINYJUICE_TEST_MODULE to an explicitly built cdylib"
    );
    std::thread::Builder::new()
        .stack_size(medulla::tokio_tuning::WORKER_STACK_BYTES)
        .spawn(enabled_scenario)
        .unwrap()
        .join()
        .unwrap();
}

fn enabled_scenario() {
    medulla::tokio_tuning::build_runtime()
        .unwrap()
        .block_on(async {
            let (requests, home, _cwd) = run_fixture(std::sync::Arc::new(
                EmbedHost::default().with_compression_enabled(true),
            ))
            .await;
            let completions: Vec<_> = requests
                .iter()
                .filter(|request| {
                    request.method == wiremock::http::Method::POST
                        && request.url.path() == "/v1/chat/completions"
                })
                .collect();
            assert!(
                completions.len() >= 2,
                "expected a second provider completion request"
            );
            let second: Value = serde_json::from_slice(&completions[1].body).unwrap();
            let mut contents = Vec::new();
            tool_contents(&second, &mut contents);
            let sent = contents.join("\n");
            assert!(
                sent.len() < FIXTURE.len(),
                "module left full output unchanged: sent {} of {}, recovery tool {}, bodies={:?}",
                sent.len(),
                FIXTURE.len(),
                requests.iter().any(
                    |request| String::from_utf8_lossy(&request.body).contains("juice_retrieve")
                ),
                completions
                    .iter()
                    .map(|r| (r.url.path().to_owned(), r.body.len()))
                    .collect::<Vec<_>>()
            );
            assert!(
                requests.iter().any(
                    |request| String::from_utf8_lossy(&request.body).contains("juice_retrieve")
                ),
                "recovery tool not advertised"
            );
            let env = HashMap::from([(
                "MEDULLA_HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]);
            let workspace = medulla::home::medulla_home(&env).join("embed/workspace");
            let stats = medulla::juice::read_embedded_stats(&workspace)
                .unwrap()
                .expect("persisted savings");
            assert!(stats.events > 0);
            assert!(stats.tokens.saved_tokens() > 0);
        });
}

#[test]
#[ignore = "run in its own process to isolate TINYJUICE_TEST_MODULE"]
fn unavailable_module_fails_open_and_preserves_the_model_visible_result() {
    std::env::set_var(
        "TINYJUICE_TEST_MODULE",
        "/definitely/missing/libtinyjuice_module.so",
    );
    std::thread::Builder::new()
        .stack_size(medulla::tokio_tuning::WORKER_STACK_BYTES)
        .spawn(unavailable_module_scenario)
        .unwrap()
        .join()
        .unwrap();
}

fn unavailable_module_scenario() {
    medulla::tokio_tuning::build_runtime()
        .unwrap()
        .block_on(async {
            let (requests, _home, _cwd) = run_fixture(std::sync::Arc::new(
                EmbedHost::default().with_compression_enabled(true),
            ))
            .await;
            let completions: Vec<_> = requests
                .iter()
                .filter(|request| {
                    request.method == wiremock::http::Method::POST
                        && request.url.path() == "/v1/chat/completions"
                })
                .collect();
            assert!(
                completions.len() >= 2,
                "expected the follow-up model request"
            );
            let second: Value = serde_json::from_slice(&completions[1].body).unwrap();
            let mut contents = Vec::new();
            tool_contents(&second, &mut contents);
            assert!(contents.iter().any(|content| content == FIXTURE));
        });
}
