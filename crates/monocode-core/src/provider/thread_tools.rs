use super::*;
use std::io::Write;

pub(super) fn enabled(request: &Request) -> bool {
    !request.settings.text_only
        && request
            .settings
            .environment
            .get("MONOCODE_THREADS")
            .is_some_and(|v| v == "1")
}

pub(super) fn pi_extension(request: &Request) -> Result<Option<tempfile::NamedTempFile>, String> {
    if !request.kind.is_pi() || !enabled(request) {
        return Ok(None);
    }
    let mut extension = tempfile::Builder::new()
        .prefix("monocode-threads-")
        .suffix(".mjs")
        .tempfile()
        .map_err(|e| e.to_string())?;
    let definitions = serde_json::to_string(&crate::visual_bridge::thread_tool_definitions())
        .map_err(|e| e.to_string())?;
    let source =
        include_str!("thread_tools.mjs").replace("/* TOOL_DEFINITIONS */ []", &definitions);
    extension
        .write_all(source.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(Some(extension))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extensions_contain_no_credentials_and_children_get_no_extension() {
        let mut request = Request {
            kind: Kind::Pi,
            binary: "pi".into(),
            cwd: "/project".into(),
            prompt: "Review".into(),
            provider_id: None,
            model: None,
            settings: Settings::default(),
        };
        assert!(pi_extension(&request).unwrap().is_none());
        request
            .settings
            .environment
            .insert("MONOCODE_THREADS".into(), "1".into());
        request
            .settings
            .environment
            .insert("MONOCODE_APP_TOKEN".into(), "private-test-token".into());
        let extension = pi_extension(&request).unwrap().unwrap();
        let source = std::fs::read_to_string(extension.path()).unwrap();
        assert!(source.contains("thread_start"));
        assert!(source.contains("registerMcpServer"));
        assert!(!source.contains("private-test-token"));
        request
            .settings
            .environment
            .insert("MONOCODE_THREADS".into(), "0".into());
        assert!(pi_extension(&request).unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn opencode_thread_tools_use_acp_and_forward_the_session_capability() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("opencode");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import json,sys
assert sys.argv[1:] == ['acp'], sys.argv
for line in sys.stdin:
    request=json.loads(line)
    method=request.get('method')
    result={}
    if method == 'initialize':
        result={'protocolVersion':1,'agentCapabilities':{},'authMethods':[]}
    elif method == 'session/new':
        server=request['params']['mcpServers'][0]
        assert server['name']=='monocode'
        env={entry['name']:entry['value'] for entry in server['env']}
        assert env['MONOCODE_APP_SESSION']=='7'
        assert env['MONOCODE_THREADS']=='1'
        result={'sessionId':'fixture-session'}
    elif method == 'session/prompt':
        result={'stopReason':'end_turn'}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut settings = Settings::default();
        for (key, value) in [
            ("MONOCODE_THREADS", "1"),
            ("MONOCODE_APP_SESSION", "7"),
            ("MONOCODE_VISUAL_ENDPOINT", "127.0.0.1:1"),
            ("MONOCODE_VISUAL_TOKEN", "fixture"),
            ("MONOCODE_VISUAL_SESSION", "7"),
            ("MONOCODE_VISUAL_EXECUTABLE", "/fixture/app"),
        ] {
            settings.environment.insert(key.into(), value.into());
        }
        let request = Request {
            kind: Kind::OpenCode,
            binary,
            cwd: directory.path().into(),
            prompt: "Review".into(),
            provider_id: None,
            model: None,
            settings,
        };
        let mut events = Vec::new();
        run(request, Arc::new(AtomicBool::new(false)), |batch| {
            events.extend(batch);
            Ok(())
        })
        .unwrap();
        assert!(events.contains(&Event::Done));
        forget_session(
            Kind::OpenCode,
            "fixture-session",
            crate::accounts::DEFAULT,
            directory.path(),
        );
    }
}
