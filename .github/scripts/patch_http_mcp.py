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
anchor = '''    if let Err(error) = validate_protocol_headers(&headers, &request) {\n        return protocol_error(StatusCode::BAD_REQUEST, request.id, -32020, error);\n    }'''
replacement = '''    let header_version = header_text(&headers, MCP_PROTOCOL_HEADER);\n    if header_version != Some(MCP_PROTOCOL_VERSION) {\n        return unsupported_protocol_error(\n            request.id,\n            header_version.unwrap_or("missing"),\n        );\n    }\n    if let Err(error) = validate_protocol_headers(&headers, &request) {\n        return protocol_error(StatusCode::BAD_REQUEST, request.id, -32020, error);\n    }'''
text = replace_once(text, anchor, replacement, 'protocol header handling')
old = '''    let protocol = header_text(headers, MCP_PROTOCOL_HEADER)\n        .ok_or_else(|| "missing MCP-Protocol-Version header".to_owned())?;\n    if protocol != MCP_PROTOCOL_VERSION {\n        return Err(format!(\n            "unsupported MCP protocol version {protocol}; supported={MCP_PROTOCOL_VERSION}"\n        ));\n    }\n    let method = header_text(headers, MCP_METHOD_HEADER)'''
new = '''    let method = header_text(headers, MCP_METHOD_HEADER)'''
text = replace_once(text, old, new, 'remove duplicate protocol check')
anchor = '''fn protocol_error(\n    status: StatusCode,'''
helper = '''fn unsupported_protocol_error(id: Value, received: &str) -> Response {\n    (\n        StatusCode::BAD_REQUEST,\n        Json(json!({\n            "jsonrpc": "2.0",\n            "id": id,\n            "error": {\n                "code": -32022,\n                "message": format!(\n                    "unsupported MCP protocol version {received}; supported={MCP_PROTOCOL_VERSION}"\n                ),\n                "data": {"supported": [MCP_PROTOCOL_VERSION]}\n            }\n        })),\n    )\n        .into_response()\n}\n\n'''
text = replace_once(text, anchor, helper + anchor, 'unsupported protocol helper')
text = replace_once(
    text,
    '''        .find(|key| !allowed.iter().any(|allowed| *allowed == key.as_str()))''',
    '''        .find(|key| !allowed.contains(&key.as_str()))''',
    'strict clippy contains',
)
mcp.write_text(text)
