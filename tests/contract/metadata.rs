use rmcp::ServerHandler;
use toolz::ToolzServer;

#[test]
fn public_tool_set_and_metadata_stay_bounded() {
    let server = ToolzServer::new();
    let mut tools = server.tools();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let names: Vec<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "dependency_trace",
            "diff_inspect",
            "failure_extract",
            "git_status",
            "repo_read",
            "repo_search",
            "run_capture",
        ]
    );

    let info = server.get_info();
    let instructions = info.instructions.as_deref().unwrap_or_default();
    assert!(
        instructions.len() <= 512,
        "server instructions exceed 512 bytes"
    );
    for tool in &tools {
        assert!(tool.description.as_deref().unwrap_or_default().len() <= 480);
        let input = serde_json::to_vec(&tool.input_schema).expect("serialize input schema");
        assert!(
            input.len() <= 8 * 1024,
            "{} input schema is too large",
            tool.name
        );
        let output = serde_json::to_vec(&tool.output_schema).expect("serialize output schema");
        assert!(
            output.len() <= 8 * 1024,
            "{} output schema is too large",
            tool.name
        );
        assert!(
            !schema_contains_format(&tool.input_schema),
            "{} input schema contains a host-specific format",
            tool.name
        );
        if let Some(output_schema) = &tool.output_schema {
            assert!(
                !schema_contains_format(output_schema),
                "{} output schema contains a host-specific format",
                tool.name
            );
        }
        let annotations = tool.annotations.as_ref().expect("tool annotations");
        if tool.name == "run_capture" {
            assert_eq!(annotations.read_only_hint, Some(false));
            assert_eq!(annotations.destructive_hint, Some(true));
        } else {
            assert_eq!(
                annotations.read_only_hint,
                Some(true),
                "{} must be read-only",
                tool.name
            );
            assert_eq!(annotations.open_world_hint, Some(false));
        }
    }
    let list = serde_json::to_vec(&tools).expect("serialize tool list");
    assert!(list.len() <= 64 * 1024, "list_tools payload is too large");
    assert_eq!(
        blake3::hash(&list).to_hex().as_str(),
        "59707cd818326309a35c876618958f6f3f9a73160474cf90ff229813bb904f12",
        "public MCP contract changed; review it and update this snapshot deliberately"
    );
}

fn schema_contains_format(value: &impl serde::Serialize) -> bool {
    fn visit(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                object.contains_key("format") || object.values().any(visit)
            }
            serde_json::Value::Array(values) => values.iter().any(visit),
            _ => false,
        }
    }

    let value = serde_json::to_value(value).expect("serialize schema value");
    visit(&value)
}
