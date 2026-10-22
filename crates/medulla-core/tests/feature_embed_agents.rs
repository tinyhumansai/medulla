//! Native turns preserve route, cwd, environment, hooks, usage and isolation.

#![recursion_limit = "512"]

use medulla::daemon::providers::{run_local_task, Abort, EmbedHost, RunTaskOptions, RunTaskOrigin};
use medulla::protocol::{HarnessProvider, HarnessTransport};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// OpenHuman's native embedding keeps an account/runtime singleton in-process,
// so scenarios that start it must not overlap within this integration-test
// binary. The scenarios still run on their own large-stack worker threads.
static EMBED_RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

async fn wait_for_pid(path: &std::path::Path) -> u32 {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(contents) = tokio::fs::read_to_string(path).await {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    return pid;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("PID file {} was not populated", path.display()))
}

#[cfg(target_os = "linux")]
fn current_process_records() -> Vec<medulla::resources::agents::ProcessRecord> {
    use medulla::resources::agents::{ProcessRecord, ProcessUsage};
    use sysinfo::{ProcessesToUpdate, System};

    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
        .processes()
        .values()
        .map(|process| ProcessRecord {
            pid: process.pid().as_u32(),
            ppid: process.parent().map(|pid| pid.as_u32()),
            exe: process.exe().map(std::path::Path::to_path_buf),
            argv: Some(
                process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            ),
            cwd: process.cwd().map(std::path::Path::to_path_buf),
            started_at_secs: process.start_time(),
            usage: ProcessUsage::default(),
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn process_tree_contains(node: &medulla::resources::agents::ProcessNode, pid: u32) -> bool {
    node.pid == pid
        || node
            .children
            .iter()
            .any(|child| process_tree_contains(child, pid))
}

#[tokio::test]
async fn pid_reader_waits_until_an_existing_empty_file_is_populated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.pid");
    std::fs::write(&path, "").unwrap();
    let writer_path = path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        std::fs::write(writer_path, "12345\n").unwrap();
    });

    assert_eq!(wait_for_pid(&path).await, 12_345);
}

#[derive(Clone)]
struct Provider;
impl Respond for Provider {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let history = body["messages"].to_string();
        let completed = history.contains("[Tool results]")
            || body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool");
        let command = if history.contains("slow-worker") {
            "sleep 60 & child=$!; printf '%s\\n' \"$child\" > child.pid.tmp && mv child.pid.tmp child.pid; wait"
        } else {
            "test -z \"$MEDULLA_TOKEN$OPENROUTER_API_KEY$FIXTURE_API_KEY\" || exit 9; echo credentials-absent; pwd; printf '\\nowner=%s\\n' \"$TURN_OWNER\""
        };
        let message = if completed {
            json!({"role":"assistant", "content":"done"})
        } else {
            json!({"role":"assistant", "content":null,"tool_calls":[{"id":"call_env","type":"function","function":{
            "name":"shell", "arguments":json!({"command":command}).to_string()}}]})
        };
        ResponseTemplate::new(200).set_body_json(json!({"id":"mock", "object":"chat.completion", "created":1700000000,
            "model":"fixture", "choices":[{"index":0,"message":message,"finish_reason":if completed {"stop"} else {"tool_calls"}}],
            "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"cost":0.01}}))
    }
}

fn options(
    host: Arc<EmbedHost>,
    home: &std::path::Path,
    cwd: &std::path::Path,
    server: &MockServer,
    owner: &str,
) -> RunTaskOptions {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("MEDULLA_HOME".into(), home.to_string_lossy().into_owned());
    env.insert("MEDULLA_TOKEN".into(), "private-medulla-bearer".into());
    env.insert("OPENROUTER_API_KEY".into(), "private-router-key".into());
    env.insert("FIXTURE_API_KEY".into(), "fixture".into());
    env.insert("TURN_OWNER".into(), owner.into());
    let mut hooks = medulla::harness_hooks::HooksConfig::default();
    hooks.hooks.push(
        serde_json::from_value(
            json!({"event":"PostToolUse","matcher":"shell","type":"command",
        "command":"cat > hook-payload.json", "harnesses":["openhuman"]}),
        )
        .unwrap(),
    );
    RunTaskOptions {
        task_id: None,
        embed: host,
        budget: None,
        provider: HarnessProvider::Openhuman,
        origin: RunTaskOrigin::Workflow,
        transport: HarnessTransport::Cli,
        prompt: format!("Check environment for {owner}."),
        cwd: cwd.to_string_lossy().into_owned(),
        env,
        timeout_ms: 10_000,
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
            base_url: Some(format!("{}/v1", server.uri())),
            api_key_env: Some("FIXTURE_API_KEY".into()),
            ..Default::default()
        }),
        attribution: false,
        hooks,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

#[test]
fn native_agents_keep_each_workers_context_private_and_resume_their_own_session() {
    let _runtime_lock = EMBED_RUNTIME_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::thread::Builder::new()
        .stack_size(medulla::tokio_tuning::WORKER_STACK_BYTES)
        .spawn(native_agents_scenario)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn embedded_openhuman_shell_process_is_nested_under_a_pidless_row() {
    let _runtime_lock = EMBED_RUNTIME_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::thread::Builder::new()
        .stack_size(medulla::tokio_tuning::WORKER_STACK_BYTES)
        .spawn(embedded_openhuman_shell_scenario)
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(target_os = "linux")]
fn embedded_openhuman_shell_scenario() {
    medulla::tokio_tuning::build_runtime()
        .unwrap()
        .block_on(async {
            let mock = MockServer::start().await;
            Mock::given(wiremock::matchers::method("POST"))
                .and(wiremock::matchers::path("/v1/chat/completions"))
                .respond_with(Provider)
                .mount(&mock)
                .await;
            let home = tempfile::tempdir().unwrap();
            let cwd = tempfile::tempdir().unwrap();
            let host = Arc::new(EmbedHost::default());
            let options = options(host, home.path(), cwd.path(), &mock, "slow-worker");
            let abort = options.abort.clone();
            let run = tokio::spawn(run_local_task(options));
            let child_pid = wait_for_pid(&cwd.path().join("child.pid")).await;

            let agents = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let processes = current_process_records();
                    let agents = medulla::resources::agents::discover_agents(
                        &processes,
                        &[],
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    );
                    if agents.iter().any(|agent| {
                        agent.kind == medulla::resources::agents::AgentKind::EmbeddedOpenhuman
                    }) {
                        break agents;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("shell child should be visible under Medulla's in-process row");

            let embedded = agents
                .iter()
                .filter(|agent| {
                    agent.kind == medulla::resources::agents::AgentKind::EmbeddedOpenhuman
                })
                .collect::<Vec<_>>();
            assert_eq!(embedded.len(), 1);
            assert_eq!(embedded[0].pid, None);
            assert!(embedded[0]
                .children
                .iter()
                .any(|child| process_tree_contains(child, child_pid)));

            abort.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), run).await;
        });
}

fn native_agents_scenario() {
    medulla::tokio_tuning::build_runtime()
        .unwrap()
        .block_on(async {
            let mock = MockServer::start().await;
            Mock::given(wiremock::matchers::method("POST"))
                .and(wiremock::matchers::path("/v1/chat/completions"))
                .respond_with(Provider)
                .mount(&mock)
                .await;
            let home = tempfile::tempdir().unwrap();
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let host = Arc::new(EmbedHost::default());
            let mut oa = options(host.clone(), home.path(), a.path(), &mock, "worker-a");
            let mut ob = options(host.clone(), home.path(), b.path(), &mock, "worker-b");
            let events = Arc::new(Mutex::new(Vec::new()));
            for options in [&mut oa, &mut ob] {
                let events = events.clone();
                options.on_event = Some(Box::new(move |event| {
                    events.lock().unwrap().push(event.event.clone())
                }));
            }
            let (ra, rb) = tokio::join!(run_local_task(oa), run_local_task(ob));
            let ra = ra.unwrap();
            let rb = rb.unwrap();
            assert_eq!(ra.reply, "done");
            assert_eq!(rb.reply, "done");
            assert_ne!(ra.session_id, rb.session_id);
            assert_eq!(ra.usage.unwrap().input_tokens, 6);
            // A second account cannot borrow the process singleton's credentials
            // or native session store, even through a separately constructed host.
            let other_home = tempfile::tempdir().unwrap();
            let before_account_switch = mock.received_requests().await.unwrap().len();
            for other_host in [host.clone(), Arc::new(EmbedHost::default())] {
                let error = run_local_task(options(other_host, other_home.path(), a.path(), &mock, "other-account"))
                    .await.unwrap_err();
                assert!(error.contains("account"), "{error}");
            }
            assert_eq!(mock.received_requests().await.unwrap().len(), before_account_switch);

            for (dir, owner) in [(a.path(), "worker-a"), (b.path(), "worker-b")] {
                let payload: Value = serde_json::from_str(
                    &std::fs::read_to_string(dir.join("hook-payload.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(payload["cwd"], dir.to_string_lossy().as_ref());
                let output = payload["tool_response"].as_str().unwrap();
                assert!(
                    output.contains(&format!("owner={owner}")) && output.contains("credentials-absent"),
                    "{output}"
                );
                assert!(!output.contains("private-medulla-bearer"));
                assert!(!output.contains("private-router-key"));
            }
            let mut resumed = options(host.clone(), home.path(), a.path(), &mock, "worker-a");
            resumed.prompt = "Continue the earlier task.".into();
            resumed.resume_session_id = ra.session_id.clone();
            let resumed = run_local_task(resumed).await.unwrap();
            assert_eq!(resumed.session_id, ra.session_id);
            let requests = mock.received_requests().await.unwrap();
            assert!(requests.iter().any(|request| request
                .body
                .windows(b"Continue the earlier task".len())
                .any(|text| text == b"Continue the earlier task")
                && request
                    .body
                    .windows(b"Check environment for worker-a".len())
                    .any(|text| text == b"Check environment for worker-a")));
            assert!(!events.lock().unwrap().is_empty());
            // A resumed turn must not recover revoked workflow declarations
            // from its persisted model/tool history.
            let before = mock.received_requests().await.unwrap().len();
            use sha2::{Digest, Sha256};
            let native_id = format!("worker-{}", &format!("{:x}", Sha256::digest(ra.session_id.as_ref().unwrap().as_bytes()))[..56]);
            let stale_skill = medulla::home::medulla_home(&options(host.clone(), home.path(), a.path(), &mock, "worker-a").env).join("embed/workspace/agents").join(native_id).join("skills/stale/SKILL.md");
            assert!(stale_skill.parent().unwrap().parent().unwrap().parent().unwrap().is_dir(), "seed the existing native agent home");
            std::fs::create_dir_all(stale_skill.parent().unwrap()).unwrap();
            std::fs::write(&stale_skill, "---\nname: stale\ndescription: Stale workflow snapshot\n---\nUse workflow_run").unwrap();
            let mut withheld = options(host.clone(), home.path(), a.path(), &mock, "worker-a");
            withheld.resume_session_id = ra.session_id.clone();
            medulla::harness_tools::withhold(&mut withheld.env);
            run_local_task(withheld).await.unwrap();
            assert!(!stale_skill.exists(), "revoked workflow skills must not survive a resumed native turn");
            let requests = mock.received_requests().await.unwrap();
            let offered = |request: &Request| -> Vec<String> {
                if request.method != wiremock::http::Method::POST { return Vec::new(); }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                body["tools"].as_array().unwrap().iter().filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned)).collect()
            };
            assert!(requests[..before].iter().any(|request| offered(request).iter().any(|name| name.starts_with("workflow_"))));
            assert!(requests[before..].iter().all(|request| offered(request).iter().all(|name| !name.starts_with("workflow_"))));

        // The daemon's ordinary input channel answers an inline permission.
        let mut approved = options(host.clone(), home.path(), a.path(), &mock, "approved");
        approved.origin = RunTaskOrigin::Interactive;
        let input = Arc::new(Mutex::new(None::<tokio::sync::mpsc::UnboundedSender<String>>));
        let captured = input.clone();
        approved.on_stdin = Some(Box::new(move |sender| *captured.lock().unwrap() = Some(sender)));
        approved.on_event = Some(Box::new(move |event| {
            if event.event.kind == "approval_request" {
                input.lock().unwrap().as_ref().unwrap().send(json!({"call_id":event.event.payload["call_id"],"decision":"allow"}).to_string()).unwrap();
            }
        }));
        assert_eq!(run_local_task(approved).await.unwrap().reply, "done");
        std::fs::remove_file(a.path().join("hook-payload.json")).unwrap();
        let mut denied = options(host.clone(), home.path(), a.path(), &mock, "denied");
        denied.origin = RunTaskOrigin::Interactive;
        let input = Arc::new(Mutex::new(None::<tokio::sync::mpsc::UnboundedSender<String>>));
        let captured = input.clone();
        denied.on_stdin = Some(Box::new(move |sender| *captured.lock().unwrap() = Some(sender)));
        denied.on_event = Some(Box::new(move |event| {
            if event.event.kind == "approval_request" {
                input.lock().unwrap().as_ref().unwrap().send(json!({"call_id":event.event.payload["call_id"],"decision":"deny"}).to_string()).unwrap();
            }
        }));
        assert!(!run_local_task(denied).await.unwrap_err().is_empty());
        assert!(!a.path().join("hook-payload.json").exists());
        // Explicit permission bypass works without an attached input surface.
        let mut bypassed = options(host.clone(), home.path(), a.path(), &mock, "bypassed");
        bypassed.origin = RunTaskOrigin::Interactive;
        bypassed.skip_permissions = true;
        assert_eq!(run_local_task(bypassed).await.unwrap().reply, "done");
        #[cfg(target_os = "linux")]
        for timeout in [false, true] {
            let slow_dir = tempfile::tempdir().unwrap();
            let mut slow = options(host.clone(), home.path(), slow_dir.path(), &mock, "slow-worker");
            // Instrumented CI may spend longer preparing the model/tool belt.
            // Keep the idle window long enough to reach the live child first.
            slow.timeout_ms = if timeout { 5_000 } else { 10_000 };
            let abort = slow.abort.clone();
            let pending = tokio::spawn(run_local_task(slow));
            let pid = wait_for_pid(&slow_dir.path().join("child.pid")).await;
            if !timeout { abort.abort(); }
            let error = tokio::time::timeout(std::time::Duration::from_secs(15), pending).await.unwrap().unwrap().unwrap_err();
            assert!(if timeout { error.contains("idle") } else { error.contains("aborted") }, "{error}");
            // Orphaned descendants can remain zombies until init reaps them;
            // they must be dead when the Medulla acknowledgement returns.
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                assert!(stat.split(") ").nth(1).unwrap().starts_with('Z'), "descendant still running: {stat}");
            }
        }


        #[cfg(target_os = "linux")]
        {
            let dir = tempfile::tempdir().unwrap();
            let mut stopped = options(host.clone(), home.path(), dir.path(), &mock, "stop-hook");
            stopped.hooks.hooks = vec![serde_json::from_value(json!({"event":"Stop","type":"command",
                "command":"sleep 60 & child=$!; printf '%s\\n' \"$child\" > stop.pid.tmp && mv stop.pid.tmp stop.pid; wait","harnesses":["openhuman"]})).unwrap()];
            let abort = stopped.abort.clone();
            let pending = tokio::spawn(run_local_task(stopped));
            let pid = wait_for_pid(&dir.path().join("stop.pid")).await;
            abort.abort();
            assert!(tokio::time::timeout(std::time::Duration::from_secs(5), pending).await.unwrap().unwrap().unwrap_err().contains("aborted during Stop hook"));
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                assert!(stat.split(") ").nth(1).unwrap().starts_with('Z'), "Stop hook descendant still running: {stat}");
            }
        }
        // Configured host budgets are charged after each model call and
        // persist across workers sharing the daemon host.
        let bounded = Arc::new(EmbedHost::with_budget(Some(serde_json::from_value(json!({
            "providers":{"openhuman":{"remainingTokens":5}}
        })).unwrap())));
        let before = mock.received_requests().await.unwrap().len();
        let result = run_local_task(options(bounded.clone(), home.path(), a.path(), &mock, "bounded")).await.unwrap();
        assert_eq!(result.usage.unwrap().input_tokens, 3);
        assert_eq!(mock.received_requests().await.unwrap().len(), before + 1);
        assert!(run_local_task(options(bounded, home.path(), a.path(), &mock, "bounded-again")).await.unwrap_err().contains("budget exhausted before start"));
        assert_eq!(mock.received_requests().await.unwrap().len(), before + 1);
        let mut exhausted = options(host.clone(),home.path(),a.path(),&mock,"exhausted");
        exhausted.budget = Some(serde_json::from_value(json!({"seatId":"test-seat","provider":"openhuman","plan":"fixture",
            "planLabel":"Fixture","headroomTokens":0,"exhausted":true,"primaryResetsAt":"2099-01-01T00:00:00Z"})).unwrap());
        let before = mock.received_requests().await.unwrap().len();
        assert!(run_local_task(exhausted).await.unwrap_err().contains("budget exhausted before start"));
        assert_eq!(mock.received_requests().await.unwrap().len(),before);

        });
}
