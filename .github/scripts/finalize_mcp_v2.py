from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new)

lib = Path('crates/aidememo-server/src/lib.rs')
text = lib.read_text()
text = replace_once(text, 'mod lexical;\nmod product;', 'mod lexical;\nmod mcp;\nmod product;', 'mcp module')
text = replace_once(
    text,
    '        .merge(product::routes())\n        .layer(DefaultBodyLimit::max(MAX_COMMAND_BODY_BYTES))',
    '        .merge(product::routes())\n        .merge(mcp::routes())\n        .layer(DefaultBodyLimit::max(MAX_COMMAND_BODY_BYTES))',
    'mcp routes',
)
lib.write_text(text)

mcp = Path('crates/aidememo-server/src/mcp.rs')
text = mcp.read_text()
text = replace_once(
    text,
    'ensure_only(args, &["q", "source_id", "limit", "at_least_seq"])?;',
    'ensure_only(args, &["q", "source_id", "limit", "at_least_seq", "mode"])?;',
    'search mode allowlist',
)
text = replace_once(
    text,
    '''    push_optional_u64(&mut query, args, "at_least_seq")?;\n    Ok(dispatch(''',
    '''    push_optional_u64(&mut query, args, "at_least_seq")?;\n    push_optional_string(&mut query, args, "mode")?;\n    Ok(dispatch(''',
    'search mode query',
)
text = replace_once(
    text,
    '''            tool_handoff_accept(),\n            tool_handoff_return(),''',
    '''            tool_handoff_accept(),\n            tool_handoff_heartbeat(),\n            tool_handoff_return(),''',
    'heartbeat tool list',
)
text = replace_once(
    text,
    '''        "handoff_accept" => dispatch_handoff_accept(state, project_id, authorization, arguments).await?,\n        "handoff_return" => dispatch_handoff_return(state, project_id, authorization, arguments).await?,''',
    '''        "handoff_accept" => dispatch_handoff_accept(state, project_id, authorization, arguments).await?,\n        "handoff_heartbeat" => {\n            dispatch_handoff_heartbeat(state, project_id, authorization, arguments).await?\n        }\n        "handoff_return" => dispatch_handoff_return(state, project_id, authorization, arguments).await?,''',
    'heartbeat call dispatch',
)
return_anchor = 'async fn dispatch_handoff_return(\n'
heartbeat_dispatch = '''async fn dispatch_handoff_heartbeat(\n    state: &ServerState,\n    project_id: &ProjectId,\n    authorization: &HeaderValue,\n    args: &Map<String, Value>,\n) -> Result<Response, McpFailure> {\n    ensure_only(\n        args,\n        &["operation_id", "handoff_id", "expected_revision", "claim_id"],\n    )?;\n    let handoff_id = required_path_segment(args, "handoff_id")?;\n    let body = json!({\n        "command_id": required_identifier(args, "operation_id")?,\n        "expected_revision": required_u64(args, "expected_revision")?,\n        "payload": {"claim_id": required_identifier(args, "claim_id")?}\n    });\n    Ok(dispatch(\n        state,\n        Method::POST,\n        &format!(\n            "/v1/projects/{}/handoffs/{handoff_id}/heartbeat",\n            project_id.as_str()\n        ),\n        authorization,\n        Some(body),\n    )\n    .await)\n}\n\n'''
text = replace_once(text, return_anchor, heartbeat_dispatch + return_anchor, 'heartbeat dispatch fn')
return_tool_anchor = 'fn tool_handoff_return() -> Value {\n'
heartbeat_tool = '''fn tool_handoff_heartbeat() -> Value {\n    write_tool(\n        "handoff_heartbeat",\n        "Renew handoff claim lease",\n        "Renew the active receiving claim using optimistic revision and the current claim id.",\n        json!({\n            "operation_id": {"type": "string"},\n            "handoff_id": {"type": "string"},\n            "expected_revision": {"type": "integer", "minimum": 1},\n            "claim_id": {"type": "string"}\n        }),\n        &["operation_id", "handoff_id", "expected_revision", "claim_id"],\n    )\n}\n\n'''
text = replace_once(text, return_tool_anchor, heartbeat_tool + return_tool_anchor, 'heartbeat tool schema')
text = replace_once(
    text,
    '''                "at_least_seq": {"type": "integer", "minimum": 0}\n            }),''',
    '''                "at_least_seq": {"type": "integer", "minimum": 0},\n                "mode": {"type": "string", "enum": ["auto", "lexical", "semantic", "hybrid"]}\n            }),''',
    'search mode schema',
)
mcp.write_text(text)

test = Path('crates/aidememo-server/tests/mcp_http.rs')
text = test.read_text()
text = replace_once(
    text,
    'const READER_TOKEN: &str = "mcp-reader-token-0123456789";\n',
    'const READER_TOKEN: &str = "mcp-reader-token-0123456789";\nconst RECEIVER_TOKEN: &str = "mcp-receiver-token-0123456789";\n',
    'receiver token',
)
reader_provision = '''    provision(\n        &mut store,\n        &tenant,\n        &project,\n        "reader",\n        MembershipRole::Reader,\n        READER_TOKEN,\n        timestamp,\n    )?;\n'''
text = replace_once(
    text,
    reader_provision,
    reader_provision + '''    provision(\n        &mut store,\n        &tenant,\n        &project,\n        "receiver",\n        MembershipRole::Writer,\n        RECEIVER_TOKEN,\n        timestamp,\n    )?;\n''',
    'receiver provision',
)
text = replace_once(
    text,
    '    assert!(writer_names.contains(&"handoff_return"));\n',
    '    assert!(writer_names.contains(&"handoff_heartbeat"));\n    assert!(writer_names.contains(&"handoff_return"));\n',
    'writer heartbeat tool assertion',
)
text = replace_once(
    text,
    '''                "arguments": {"q": "redis", "source_id": "alpha", "at_least_seq": head},''',
    '''                "arguments": {"q": "redis", "source_id": "alpha", "at_least_seq": head, "mode": "lexical"},''',
    'MCP search mode',
)
text = replace_once(
    text,
    '''                "/v1/projects/project_mcp/search?q=redis&source_id=alpha&at_least_seq={head}"''',
    '''                "/v1/projects/project_mcp/search?q=redis&source_id=alpha&at_least_seq={head}&mode=lexical"''',
    'REST search mode parity',
)
append = r'''

#[tokio::test]
async fn heartbeat_tool_renews_the_same_canonical_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let app = test_app()?;
    let session = app
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            Some("session_create"),
            Some(WRITER_TOKEN),
            json!({
                "name": "session_create",
                "arguments": {
                    "operation_id": "mcp-heartbeat-session-op",
                    "session_id": "mcp-heartbeat-session",
                    "source_id": "heartbeat",
                    "topic": "MCP heartbeat"
                },
                "_meta": meta()
            }),
        )?)
        .await?;
    assert_eq!(session.status(), 200);
    assert_eq!(response_json(session).await?["result"]["isError"], false);

    let sent = app
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            Some("handoff_send"),
            Some(WRITER_TOKEN),
            json!({
                "name": "handoff_send",
                "arguments": {
                    "operation_id": "mcp-heartbeat-send-op",
                    "handoff_id": "mcp-heartbeat-handoff",
                    "session_id": "mcp-heartbeat-session",
                    "to_actor": "receiver",
                    "focus": "renew lease"
                },
                "_meta": meta()
            }),
        )?)
        .await?;
    let sent = response_json(sent).await?;
    assert_eq!(sent["result"]["isError"], false);
    let sent_revision = sent["result"]["structuredContent"]["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("send revision missing"))?;

    let accepted = app
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            Some("handoff_accept"),
            Some(RECEIVER_TOKEN),
            json!({
                "name": "handoff_accept",
                "arguments": {
                    "operation_id": "mcp-heartbeat-accept-op",
                    "handoff_id": "mcp-heartbeat-handoff",
                    "expected_revision": sent_revision,
                    "claim_id": "mcp-heartbeat-claim"
                },
                "_meta": meta()
            }),
        )?)
        .await?;
    let accepted = response_json(accepted).await?;
    assert_eq!(accepted["result"]["isError"], false);
    let accepted_revision = accepted["result"]["structuredContent"]["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("accept revision missing"))?;

    let heartbeat = app
        .oneshot(mcp_request(
            "tools/call",
            Some("handoff_heartbeat"),
            Some(RECEIVER_TOKEN),
            json!({
                "name": "handoff_heartbeat",
                "arguments": {
                    "operation_id": "mcp-heartbeat-renew-op",
                    "handoff_id": "mcp-heartbeat-handoff",
                    "expected_revision": accepted_revision,
                    "claim_id": "mcp-heartbeat-claim"
                },
                "_meta": meta()
            }),
        )?)
        .await?;
    let heartbeat = response_json(heartbeat).await?;
    assert_eq!(heartbeat["result"]["isError"], false);
    assert_eq!(
        heartbeat["result"]["structuredContent"]["revision"],
        accepted_revision + 1
    );
    Ok(())
}
'''
if 'heartbeat_tool_renews_the_same_canonical_claim' not in text:
    text += append
test.write_text(text)
