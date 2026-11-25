use super::auth::*;
use super::frame::*;
use super::protocol::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Cursor;

const SCHEMA_TEXT: &str =
    include_str!("../../../../../../extensions/zptah-browser/shared/protocol.json");
const VECTORS_TEXT: &str =
    include_str!("../../../../../../extensions/zptah-browser/shared/handshake-vectors.json");
const VECTORS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../extensions/zptah-browser/shared/handshake-vectors.json"
);

fn schema() -> Value {
    serde_json::from_str(SCHEMA_TEXT).expect("protocol.json is valid JSON")
}

// ---- a small validator for the JSON Schema subset protocol.json uses ----

fn resolve<'a>(root: &'a Value, reference: &str) -> &'a Value {
    let path = reference.strip_prefix("#/").expect("local refs only");
    path.split('/').fold(root, |node, part| {
        node.get(part)
            .unwrap_or_else(|| panic!("bad ref {reference}"))
    })
}

fn validate(root: &Value, schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return validate(root, resolve(root, reference), value);
    }
    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let hits = options
            .iter()
            .filter(|o| validate(root, o, value).is_ok())
            .count();
        if hits != 1 {
            return Err(format!("matches {hits} oneOf branches: {value}"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            return Err(format!("{value} is not const {expected}"));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{value} is not in enum"));
        }
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let ok = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            other => panic!("validator lacks type {other}"),
        };
        if !ok {
            return Err(format!("{value} is not a {kind}"));
        }
    }
    if let Some(text) = value.as_str() {
        let chars = text.chars().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
            if chars < min {
                return Err(format!("string shorter than {min}"));
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
            if chars > max {
                return Err(format!("string longer than {max}"));
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if !regex::Regex::new(pattern).unwrap().is_match(text) {
                return Err(format!("{text:?} does not match {pattern}"));
            }
        }
    }
    if let Some(number) = value.as_i64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_i64) {
            if number < min {
                return Err(format!("{number} below {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_i64) {
            if number > max {
                return Err(format!("{number} above {max}"));
            }
        }
    }
    if let Some(map) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    return Err(format!("missing `{name}`"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (name, sub) in props {
                if let Some(member) = map.get(name) {
                    validate(root, sub, member).map_err(|e| format!("{name}: {e}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(sub)) = (value.as_array(), schema.get("items")) {
        for item in items {
            validate(root, sub, item)?;
        }
    }
    Ok(())
}

fn valid(root: &Value, def: &str, value: &Value) -> Result<(), String> {
    validate(root, &json!({ "$ref": format!("#/$defs/{def}") }), value)
}

fn assert_round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
    def: &str,
    value: &T,
) {
    let root = schema();
    let json = serde_json::to_value(value).unwrap();
    valid(&root, def, &json).unwrap_or_else(|e| panic!("{def} vs schema: {e}: {json}"));
    let back: T = serde_json::from_value(json).unwrap();
    assert_eq!(&back, value);
}

// ---- samples ----

const MONO: &str = "0123456789abcdef0123456789abcdef";
const INSTANCE: &str = "fedcba9876543210fedcba9876543210";

fn b64_of(len: usize) -> String {
    encode_bytes(&vec![7u8; len])
}

fn roster_entry() -> RosterEntry {
    RosterEntry {
        id: MONO.into(),
        name: "Mia".into(),
        emoji: "x".into(),
        color: "#aabbcc".into(),
        group_color: ChromeGroupColor::Cyan,
        level: AccessLevel::Act,
        allowed_origins: vec!["https://example.com".into()],
        blocked_origins: vec![],
        cursor: true,
    }
}

fn tab_info() -> TabInfo {
    TabInfo {
        tab: 12,
        window: 3,
        url: "https://example.com/".into(),
        title: "Example".into(),
        owner: Some(MONO.into()),
        group_title: Some("Mia".into()),
        active_in_window: false,
        audible: false,
        loading: false,
    }
}

fn hello_params() -> HelloParams {
    HelloParams {
        protocol: [1, 1],
        ext_version: "1.0.4".into(),
        browser: BrowserInfo {
            brand: "Chrome".into(),
            version: "158.0".into(),
        },
        instance_id: INSTANCE.into(),
        paired: true,
        challenge: b64_of(32),
    }
}

/// One valid params and result per method, straight from the spec tables.
fn samples() -> Vec<(Method, Value, Value)> {
    let tab = serde_json::to_value(tab_info()).unwrap();
    let empty = json!({});
    let url_title = json!({"url": "https://example.com/", "title": "Example"});
    let prepared = json!({
        "prepare_id": "p1", "expires_ms": 30000, "digest": "ab", "url": "https://example.com/",
        "title": "Example", "frame_url": "https://example.com/", "element": {"role": "button"},
        "ext_risk": ["send"]
    });
    vec![
        (
            Method::Hello,
            serde_json::to_value(hello_params()).unwrap(),
            json!({"protocol": 1, "app_version": "1.2.3", "challenge": b64_of(32), "mac": b64_of(32)}),
        ),
        (
            Method::Auth,
            json!({"mac": b64_of(32)}),
            json!({"roster": [serde_json::to_value(roster_entry()).unwrap()], "stopped": false}),
        ),
        (
            Method::PairBegin,
            json!({"nonce": encode_bytes(&[9u8; 16]), "instance_id": INSTANCE}),
            json!({"key": b64_of(32)}),
        ),
        (
            Method::RosterSet,
            json!({"monos": [serde_json::to_value(roster_entry()).unwrap()]}),
            empty.clone(),
        ),
        (
            Method::StatusGet,
            empty.clone(),
            json!({"stopped": false, "debugger_attached": [4], "monos_active": 1}),
        ),
        (Method::StopResume, empty.clone(), empty.clone()),
        (
            Method::TabsList,
            json!({"all": true}),
            json!({"tabs": [tab.clone()]}),
        ),
        (
            Method::TabOpen,
            json!({"url": "https://example.com/", "after_tab": 5}),
            tab.clone(),
        ),
        (Method::TabClose, json!({"tab": 5}), empty.clone()),
        (
            Method::TabAdopt,
            json!({"tab": 5, "grant": {"kind": "user_reference", "quote": "that tab", "message_ms": 1000}}),
            tab.clone(),
        ),
        (Method::TabRelease, json!({"tab": 5}), empty.clone()),
        (
            Method::PageSnapshot,
            json!({"tab": 5, "mode": "interactive", "scope_ref": "e3", "depth": 4, "max_bytes": 65536, "from": 0}),
            json!({"text": "- button", "url": "https://example.com/", "title": "Example",
                   "generation": 2, "truncated": false, "hidden_dropped": 0}),
        ),
        (
            Method::PageRead,
            json!({"tab": 5, "max_bytes": 1000}),
            json!({"markdown": "# Hi", "url": "https://example.com/", "title": "Example", "truncated": false}),
        ),
        (
            Method::PageFind,
            json!({"tab": 5, "query": "buy", "max": 5}),
            json!({"matches": [{"ref": "e1", "role": "button", "name": "Buy", "context": "Cart"}]}),
        ),
        (
            Method::PageScreenshot,
            json!({"tab": 5, "ref": "e1", "full_page": false}),
            json!({"png_b64": b64_of(30), "width": 800, "height": 600, "method": "visible"}),
        ),
        (
            Method::PageWait,
            json!({"tab": 5, "text": "Done", "time_ms": 1000}),
            json!({"met": true, "url": "https://example.com/"}),
        ),
        (
            Method::NavGo,
            json!({"tab": 5, "url": "https://example.com/"}),
            url_title.clone(),
        ),
        (
            Method::NavHistory,
            json!({"tab": 5, "dir": "back"}),
            url_title.clone(),
        ),
        (
            Method::ActPrepare,
            json!({"tab": 5, "action": {"kind": "click", "ref": "e1"}}),
            prepared,
        ),
        (
            Method::ActCommit,
            json!({"prepare_id": "p1", "approval": {"approval_id": "a1", "categories": ["send"], "digest": "ab"}, "input": "auto"}),
            json!({"url": "https://example.com/", "title": "Example", "effects": {}}),
        ),
        (
            Method::ActExtend,
            json!({"prepare_id": "p1", "until_ms": 60000}),
            json!({"expires_ms": 60000}),
        ),
        (
            Method::ActCancel,
            json!({"prepare_id": "p1"}),
            empty.clone(),
        ),
        (
            Method::DialogRespond,
            json!({"tab": 5, "accept": true, "prompt_text": "ok"}),
            empty.clone(),
        ),
        (
            Method::UploadPut,
            json!({"upload_id": "u1", "seq": 0, "bytes_b64": b64_of(9), "last": true}),
            empty,
        ),
        (
            Method::CursorPrefs,
            json!({}),
            json!({"enabled": true, "off_sites": ["https://a.example"]}),
        ),
    ]
}

fn event_samples() -> Vec<(Event, Value)> {
    vec![
        (Event::TabUpdated, serde_json::to_value(tab_info()).unwrap()),
        (Event::TabRemoved, json!({"tab": 5})),
        (
            Event::TabOwnership,
            json!({"tab": 5, "owner": null, "by": "user_drag"}),
        ),
        (Event::Stop, json!({"by": "toolbar"})),
        (
            Event::TabStopped,
            json!({"tab": 5, "mono": MONO, "by": "overlay"}),
        ),
        (
            Event::DialogOpened,
            json!({"tab": 5, "kind": "confirm", "message": "Sure?"}),
        ),
        (
            Event::DebuggerDetached,
            json!({"tab": 5, "reason": "canceled_by_user"}),
        ),
        (
            Event::CursorChanged,
            json!({"enabled": false, "off_sites": []}),
        ),
    ]
}

fn error_of<T: std::fmt::Debug>(r: Result<T, ProtocolError>) -> ErrorCode {
    r.expect_err("expected a protocol error").code
}

// ---- protocol ----

#[test]
fn schema_lists_exactly_the_methods_events_and_codes() {
    let root = schema();
    let names = |key: &str| {
        let mut v: Vec<String> = root[key].as_object().unwrap().keys().cloned().collect();
        v.sort();
        v
    };
    let mut methods: Vec<String> = Method::ALL.iter().map(|m| m.as_str().to_string()).collect();
    methods.sort();
    assert_eq!(names("x-methods"), methods);
    let mut events: Vec<String> = Event::ALL.iter().map(|e| e.as_str().to_string()).collect();
    events.sort();
    assert_eq!(names("x-events"), events);
    let codes: Vec<Value> = ErrorCode::ALL.iter().map(|c| json!(c.as_str())).collect();
    assert_eq!(
        root["$defs"]["ErrorCode"]["enum"].as_array().unwrap(),
        &codes
    );
    for code in ErrorCode::ALL {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        assert_eq!(
            serde_json::from_value::<ErrorCode>(json!(code.as_str())).unwrap(),
            code
        );
    }
    for method in Method::ALL {
        assert_eq!(Method::parse(method.as_str()).unwrap(), method);
    }
}

#[test]
fn every_method_round_trips_and_validates_against_the_schema() {
    let root = schema();
    for (method, params, result) in samples() {
        let spec = &root["x-methods"][method.as_str()];
        validate(&root, &spec["params"], &params)
            .unwrap_or_else(|e| panic!("{} params: {e}", method.as_str()));
        validate(&root, &spec["result"], &result)
            .unwrap_or_else(|e| panic!("{} result: {e}", method.as_str()));

        let (sender, id) = if method.extension_originated() {
            (Sender::Extension, "x-1")
        } else {
            (Sender::App, "a-1")
        };
        let mut request = Envelope::request(id, method, params);
        if method.requires_mono() {
            request = request.with_mono(MONO);
        }
        valid(&root, "Envelope", &serde_json::to_value(&request).unwrap()).unwrap();
        let back = Envelope::from_slice(&request.to_vec()).unwrap();
        assert_eq!(back, request);
        back.check_from(sender).unwrap();

        let other_id = if sender == Sender::App { "a-1" } else { "x-1" };
        let response = Envelope::response_ok(other_id, result);
        valid(&root, "Envelope", &serde_json::to_value(&response).unwrap()).unwrap();
        let back = Envelope::from_slice(&response.to_vec()).unwrap();
        assert_eq!(back, response);
        let responder = if sender == Sender::App {
            Sender::Extension
        } else {
            Sender::App
        };
        back.check_from(responder).unwrap();
    }
}

#[test]
fn every_event_validates_and_only_the_extension_may_send_it() {
    let root = schema();
    for (event, params) in event_samples() {
        validate(&root, &root["x-events"][event.as_str()]["params"], &params)
            .unwrap_or_else(|e| panic!("{} params: {e}", event.as_str()));
        let envelope = Envelope::event("x-9", event, params);
        let back = Envelope::from_slice(&envelope.to_vec()).unwrap();
        assert_eq!(back, envelope);
        back.check_from(Sender::Extension).unwrap();
        assert_eq!(
            error_of(back.check_from(Sender::App)),
            ErrorCode::Unsupported
        );
    }
}

#[test]
fn typed_payloads_round_trip_through_the_schema() {
    assert_round_trip("HelloParams", &hello_params());
    assert_round_trip(
        "HelloResult",
        &HelloResult {
            protocol: 1,
            app_version: "1.2.3".into(),
            challenge: b64_of(32),
            mac: Some(b64_of(32)),
        },
    );
    assert_round_trip(
        "HelloResult",
        &HelloResult {
            protocol: 1,
            app_version: "1.2.3".into(),
            challenge: b64_of(32),
            mac: None,
        },
    );
    assert_round_trip("AuthParams", &AuthParams { mac: b64_of(32) });
    assert_round_trip(
        "AuthResult",
        &AuthResult {
            roster: vec![roster_entry()],
            stopped: true,
        },
    );
    assert_round_trip(
        "PairBeginParams",
        &PairBeginParams {
            nonce: encode_bytes(&[1u8; 16]),
            instance_id: INSTANCE.into(),
        },
    );
    assert_round_trip("PairBeginResult", &PairBeginResult { key: b64_of(32) });
    assert_round_trip("TabInfo", &tab_info());
    assert_round_trip(
        "TabInfo",
        &TabInfo {
            owner: None,
            group_title: None,
            ..tab_info()
        },
    );
    assert_round_trip("RosterEntry", &roster_entry());
}

#[test]
fn every_action_kind_round_trips_through_the_schema() {
    let actions = vec![
        Action::Click {
            target: "e1".into(),
            button: Some("left".into()),
            double: Some(true),
            modifiers: Some(vec!["Shift".into()]),
        },
        Action::Type {
            target: "e2".into(),
            text: "hello".into(),
            submit: Some(false),
            slowly: None,
        },
        Action::Fill {
            fields: vec![FillField {
                target: "e3".into(),
                value: "a".into(),
                field_type: "text".into(),
            }],
        },
        Action::Select {
            target: "e4".into(),
            values: vec!["a".into()],
        },
        Action::Press {
            key: "Enter".into(),
            target: None,
        },
        Action::Hover {
            target: "e5".into(),
        },
        Action::Drag {
            from_ref: "e6".into(),
            to_ref: "e7".into(),
        },
        Action::Upload {
            target: "e8".into(),
            upload_id: "u1".into(),
            names: vec!["a.txt".into()],
        },
        Action::Scroll {
            target: None,
            dx: 0,
            dy: -300,
        },
    ];
    assert_eq!(actions.len(), 9);
    for action in &actions {
        assert_round_trip("Action", action);
    }
    // An unknown kind fails both the schema and the typed parse.
    let unknown = json!({"kind": "teleport", "ref": "e1"});
    assert!(valid(&schema(), "Action", &unknown).is_err());
    assert!(serde_json::from_value::<Action>(unknown).is_err());
}

#[test]
fn schema_and_parser_reject_the_same_bad_envelopes() {
    let root = schema();
    let bad = [
        json!({"v": 1, "type": "req", "id": "z-1", "method": "hello"}),
        json!({"v": 1, "type": "req", "id": "a-", "method": "hello"}),
        json!({"v": 1, "type": "req", "id": format!("a-{}", "9".repeat(31)), "method": "hello"}),
        json!({"v": 1, "type": "ping", "id": "a-1"}),
        json!({"v": 1, "type": "req", "id": "a-1", "method": "tab.close", "mono": "NOT-HEX"}),
        json!({"v": 1, "type": "req", "id": "a-1", "method": "tab.close", "params": []}),
        json!({"v": 2, "type": "req", "id": "a-1", "method": "tab.close"}),
        json!({"type": "req", "id": "a-1", "method": "tab.close"}),
    ];
    for envelope in bad {
        assert!(
            valid(&root, "Envelope", &envelope).is_err(),
            "schema accepted {envelope}"
        );
        let parsed = Envelope::from_slice(&serde_json::to_vec(&envelope).unwrap());
        assert!(parsed.is_err(), "parser accepted {envelope}");
    }
}

#[test]
fn unknown_methods_are_unsupported_and_direction_is_enforced() {
    let unknown = Envelope::request("a-1", Method::StatusGet, json!({}));
    let mut unknown = unknown;
    unknown.method = Some("page.hack".into());
    assert_eq!(
        error_of(unknown.check_from(Sender::App)),
        ErrorCode::Unsupported
    );
    // Names are exact: no case folding, no trimming.
    unknown.method = Some("Status.Get".into());
    assert_eq!(
        error_of(unknown.check_from(Sender::App)),
        ErrorCode::Unsupported
    );
    unknown.method = Some("status.get ".into());
    assert_eq!(
        error_of(unknown.check_from(Sender::App)),
        ErrorCode::Unsupported
    );

    // The extension never gets to request an app method, and the app never sends hello.
    let from_ext = Envelope::request("x-1", Method::TabClose, json!({"tab": 1})).with_mono(MONO);
    assert_eq!(
        error_of(from_ext.check_from(Sender::Extension)),
        ErrorCode::Unsupported
    );
    let from_app = Envelope::request("a-1", Method::Hello, json!({}));
    assert_eq!(
        error_of(from_app.check_from(Sender::App)),
        ErrorCode::Unsupported
    );

    let mut event = Envelope::event("x-1", Event::Stop, json!({"by": "toolbar"}));
    event.method = Some("tab.format".into());
    assert_eq!(
        error_of(event.check_from(Sender::Extension)),
        ErrorCode::Unsupported
    );
}

#[test]
fn id_prefix_and_mono_rules() {
    let wrong_prefix = Envelope::request("x-1", Method::StatusGet, json!({}));
    assert_eq!(
        error_of(wrong_prefix.check_from(Sender::App)),
        ErrorCode::Invalid
    );
    let no_mono = Envelope::request("a-1", Method::PageRead, json!({"tab": 1}));
    assert_eq!(
        error_of(no_mono.check_from(Sender::App)),
        ErrorCode::Invalid
    );
    let response_same_side = Envelope::response_ok("a-1", json!({}));
    assert_eq!(
        error_of(response_same_side.check_from(Sender::App)),
        ErrorCode::Invalid
    );
}

#[test]
fn unknown_fields_are_ignored_and_missing_required_fields_are_invalid() {
    let text = json!({
        "v": 1, "type": "req", "id": "a-4", "method": "status.get", "params": {"extra": 1},
        "future_field": {"a": [1, 2]}
    });
    let envelope = Envelope::from_slice(&serde_json::to_vec(&text).unwrap()).unwrap();
    envelope.check_from(Sender::App).unwrap();

    let mut hello = serde_json::to_value(hello_params()).unwrap();
    hello["added_later"] = json!(true);
    assert!(HelloParams::parse(&Some(hello.clone())).is_ok());
    hello.as_object_mut().unwrap().remove("instance_id");
    assert_eq!(
        error_of(HelloParams::parse(&Some(hello))),
        ErrorCode::Invalid
    );

    for broken in [
        json!({"v": 1, "type": "req", "id": "a-1"}),
        json!({"v": 1, "type": "res", "id": "x-1"}),
        json!({"v": 1, "type": "res", "id": "x-1", "ok": true}),
        json!({"v": 1, "type": "res", "id": "x-1", "ok": false}),
        json!({"v": 1, "type": "evt", "id": "x-1"}),
        json!({"v": 1, "id": "a-1"}),
        json!({"v": "1", "type": "req", "id": "a-1", "method": "status.get"}),
    ] {
        let error = error_of(Envelope::from_slice(&serde_json::to_vec(&broken).unwrap()));
        assert_eq!(error, ErrorCode::Invalid, "{broken}");
    }
}

#[test]
fn unsupported_envelope_version_is_e_protocol_with_the_supported_range() {
    let bytes = br#"{"v":2,"type":"req","id":"x-1","method":"hello"}"#;
    let error = Envelope::from_slice(bytes).unwrap_err();
    assert_eq!(error.code, ErrorCode::Protocol);
    assert_eq!(error.detail.unwrap(), json!({"supported": [1, 1]}));
}

#[test]
fn version_negotiation() {
    assert_eq!(negotiate([1, 1]).unwrap(), 1);
    assert_eq!(negotiate([1, 3]).unwrap(), 1);
    assert_eq!(negotiate([0, 1]).unwrap(), 1);
    for range in [[2, 3], [5, 9], [3, 2]] {
        let error = negotiate(range).unwrap_err();
        assert_eq!(error.code, ErrorCode::Protocol);
        assert_eq!(error.detail.unwrap(), json!({"supported": [1, 1]}));
    }
}

#[test]
fn error_responses_carry_the_code() {
    let error = ProtocolError::new(ErrorCode::NotOwner, "not yours").with_detail(json!({"tab": 4}));
    let response = Envelope::response_err("x-3", &error);
    let back = Envelope::from_slice(&response.to_vec()).unwrap();
    assert_eq!(back.ok, Some(false));
    assert_eq!(back.error.unwrap().code, ErrorCode::NotOwner);
    valid(
        &schema(),
        "Envelope",
        &serde_json::to_value(&response).unwrap(),
    )
    .unwrap();
}

// ---- framing ----

#[test]
fn frames_round_trip_in_every_format() {
    for format in [
        FrameFormat::SOCKET,
        FrameFormat::HOST_FROM_EXTENSION,
        FrameFormat::HOST_TO_EXTENSION,
    ] {
        let mut wire = Vec::new();
        write_frame(&mut wire, format, b"{\"a\":1}").unwrap();
        write_frame(&mut wire, format, b"{}").unwrap();
        let mut cursor = Cursor::new(wire);
        assert_eq!(
            read_frame(&mut cursor, format).unwrap().unwrap(),
            b"{\"a\":1}"
        );
        assert_eq!(read_frame(&mut cursor, format).unwrap().unwrap(), b"{}");
        assert!(read_frame(&mut cursor, format).unwrap().is_none());
    }
    assert_eq!(
        encode_frame(FrameFormat::SOCKET, b"ab").unwrap(),
        [2, 0, 0, 0, b'a', b'b']
    );
}

#[test]
fn oversize_frames_are_refused_before_the_payload_is_read() {
    // Only a header that claims one byte too many: nothing follows, yet the error is TooLarge.
    let header = ((MAX_SOCKET_FRAME as u32) + 1).to_le_bytes();
    let error = read_frame(&mut Cursor::new(header.to_vec()), FrameFormat::SOCKET).unwrap_err();
    assert!(matches!(error, FrameError::TooLarge { .. }), "{error}");
    let header = u32::MAX.to_le_bytes();
    let error = read_frame(&mut Cursor::new(header.to_vec()), FrameFormat::SOCKET).unwrap_err();
    assert!(matches!(error, FrameError::TooLarge { .. }));

    let big = vec![b'x'; MAX_HOST_TO_EXTENSION_FRAME + 1];
    let error = encode_frame(FrameFormat::HOST_TO_EXTENSION, &big).unwrap_err();
    assert!(matches!(error, FrameError::TooLarge { .. }));
    // Exactly at the limit is fine.
    let at_limit = vec![b'x'; MAX_HOST_TO_EXTENSION_FRAME];
    encode_frame(FrameFormat::HOST_TO_EXTENSION, &at_limit).unwrap();
}

#[test]
fn empty_and_truncated_frames_are_rejected() {
    let error = read_frame(&mut Cursor::new(vec![0, 0, 0, 0]), FrameFormat::SOCKET).unwrap_err();
    assert!(matches!(error, FrameError::Empty));
    assert!(matches!(
        encode_frame(FrameFormat::SOCKET, b""),
        Err(FrameError::Empty)
    ));
    // The stream ends inside the header.
    let error = read_frame(&mut Cursor::new(vec![5, 0]), FrameFormat::SOCKET).unwrap_err();
    assert!(matches!(error, FrameError::Truncated));
    // The stream ends inside the payload.
    let error = read_frame(
        &mut Cursor::new(vec![5, 0, 0, 0, b'a', b'b']),
        FrameFormat::SOCKET,
    )
    .unwrap_err();
    assert!(matches!(error, FrameError::Truncated));
}

#[test]
fn decoder_handles_split_and_batched_input_and_stops_on_a_bad_length() {
    let mut wire = encode_frame(FrameFormat::SOCKET, b"{\"x\":1}").unwrap();
    wire.extend(encode_frame(FrameFormat::SOCKET, b"{}").unwrap());
    let mut decoder = FrameDecoder::new(FrameFormat::SOCKET);
    for byte in &wire[..5] {
        decoder.push(&[*byte]);
        assert!(decoder.next_frame().unwrap().is_none());
    }
    decoder.push(&wire[5..]);
    assert_eq!(decoder.next_frame().unwrap().unwrap(), b"{\"x\":1}");
    assert_eq!(decoder.next_frame().unwrap().unwrap(), b"{}");
    assert!(decoder.next_frame().unwrap().is_none());
    assert_eq!(decoder.pending(), 0);

    let mut decoder = FrameDecoder::new(FrameFormat::SOCKET);
    decoder.push(&u32::MAX.to_le_bytes());
    assert!(matches!(
        decoder.next_frame(),
        Err(FrameError::TooLarge { .. })
    ));
}

#[test]
fn malformed_payloads_are_e_invalid() {
    let cases: Vec<Vec<u8>> = vec![
        b"not json".to_vec(),
        b"[1,2,3]".to_vec(),
        b"\"text\"".to_vec(),
        b"null".to_vec(),
        vec![0xff, 0xfe, 0xfd],
        b"{\"v\":1".to_vec(),
        "[".repeat(5000).into_bytes(),
    ];
    for bytes in cases {
        assert_eq!(error_of(Envelope::from_slice(&bytes)), ErrorCode::Invalid);
    }
}

// ---- auth ----

fn key(seed: u8) -> PairingKey {
    PairingKey::from_bytes([seed; KEY_LEN])
}

fn ctx<'a>(key: Option<&'a PairingKey>, pairing_open: bool) -> HelloContext<'a> {
    HelloContext {
        key,
        pairing_open,
        app_version: "2.0.0",
    }
}

/// Runs a full handshake and returns the two sessions (app side, extension side).
fn connect(k: &PairingKey) -> (Session, Session) {
    let mut ext = ExtHandshake::new(k.clone(), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let mut log = ChallengeLog::default();
    let result = app
        .on_hello(&hello, &ctx(Some(k), false), &mut log)
        .unwrap();
    let auth = ext.on_hello_result(&result).unwrap();
    let app_session = app.on_auth(&auth).unwrap();
    (app_session, ext.into_session().unwrap())
}

#[test]
fn handshake_succeeds_and_frames_flow_both_ways() {
    let (mut app, mut ext) = connect(&key(1));
    for i in 1..=3u64 {
        let mut request = Envelope::request(format!("a-{i}"), Method::StatusGet, json!({}));
        app.seal(&mut request).unwrap();
        assert_eq!(request.seq, Some(i));
        // It survives the wire.
        let received = Envelope::from_slice(&request.to_vec()).unwrap();
        ext.open(&received).unwrap();

        let mut response = Envelope::response_ok(format!("a-{i}"), json!({"stopped": false}));
        ext.seal(&mut response).unwrap();
        let received = Envelope::from_slice(&response.to_vec()).unwrap();
        app.open(&received).unwrap();
    }
}

#[test]
fn handshake_with_the_wrong_key_fails_on_both_sides() {
    // The extension holds a different key: it rejects the app's MAC.
    let mut ext = ExtHandshake::new(key(2), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let result = app
        .on_hello(
            &hello,
            &ctx(Some(&key(1)), false),
            &mut ChallengeLog::default(),
        )
        .unwrap();
    let error = ext.on_hello_result(&result).unwrap_err();
    assert_eq!(error, AuthError::BadMac);
    assert_eq!(error.code(), ErrorCode::Auth);

    // An attacker who skips the check and answers with its own key fails the app's check.
    let forged = {
        let app_chal = decode_challenge(&result.challenge).unwrap();
        let ext_chal = decode_challenge(&hello.challenge).unwrap();
        AuthParams {
            mac: encode_bytes(&ext_mac(&key(2), &ext_chal, &app_chal, INSTANCE)),
        }
    };
    let error = app.on_auth(&forged).unwrap_err();
    assert_eq!(error, AuthError::BadMac);
}

#[test]
fn auth_checks_the_instance_id_and_both_challenges() {
    let k = key(3);
    let mut ext = ExtHandshake::new(k.clone(), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let result = app
        .on_hello(&hello, &ctx(Some(&k), false), &mut ChallengeLog::default())
        .unwrap();
    let app_chal = decode_challenge(&result.challenge).unwrap();
    let ext_chal = decode_challenge(&hello.challenge).unwrap();
    // A MAC bound to another instance id, or with the challenges swapped, does not verify.
    let other_instance = "00000000000000000000000000000000";
    for mac in [
        ext_mac(&k, &ext_chal, &app_chal, other_instance),
        ext_mac(&k, &app_chal, &ext_chal, INSTANCE),
        app_mac(&k, &ext_chal, &app_chal, INSTANCE),
    ] {
        let mut app = AppHandshake::new();
        app.on_hello_with(
            &hello,
            &ctx(Some(&k), false),
            &mut ChallengeLog::default(),
            app_chal,
        )
        .unwrap();
        let error = app
            .on_auth(&AuthParams {
                mac: encode_bytes(&mac),
            })
            .unwrap_err();
        assert_eq!(error, AuthError::BadMac);
    }
}

#[test]
fn a_replayed_hello_challenge_is_refused() {
    let k = key(4);
    let mut ext = ExtHandshake::new(k.clone(), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut log = ChallengeLog::default();
    AppHandshake::new()
        .on_hello(&hello, &ctx(Some(&k), false), &mut log)
        .unwrap();
    let error = AppHandshake::new()
        .on_hello(&hello, &ctx(Some(&k), false), &mut log)
        .unwrap_err();
    assert_eq!(error, AuthError::ChallengeReplayed);
    assert_eq!(error.code(), ErrorCode::Auth);
}

#[test]
fn the_challenge_log_forgets_only_the_oldest_entries() {
    let mut log = ChallengeLog::new(2);
    log.record(&[1; 32]).unwrap();
    log.record(&[2; 32]).unwrap();
    assert!(log.record(&[2; 32]).is_err());
    log.record(&[3; 32]).unwrap();
    // [1] fell out of the window; [2] and [3] are still remembered.
    assert!(log.record(&[3; 32]).is_err());
    assert!(log.record(&[2; 32]).is_err());
    log.record(&[1; 32]).unwrap();
}

#[test]
fn handshake_steps_out_of_order_fail() {
    let k = key(5);
    let mut app = AppHandshake::new();
    let error = app.on_auth(&AuthParams { mac: b64_of(32) }).unwrap_err();
    assert_eq!(error, AuthError::OutOfOrder);

    // A second hello on one connection is refused.
    let mut ext = ExtHandshake::new(k.clone(), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let mut log = ChallengeLog::default();
    app.on_hello(&hello, &ctx(Some(&k), false), &mut log)
        .unwrap();
    let again = ExtHandshake::new(k.clone(), INSTANCE).hello(true, "1.0.0", "Chrome");
    let error = app
        .on_hello(&again, &ctx(Some(&k), false), &mut log)
        .unwrap_err();
    assert_eq!(error, AuthError::OutOfOrder);

    // Auth succeeds once; the same message again is out of order.
    let mut ext = ExtHandshake::new(k.clone(), INSTANCE);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let result = app
        .on_hello(&hello, &ctx(Some(&k), false), &mut log)
        .unwrap();
    let auth = ext.on_hello_result(&result).unwrap();
    app.on_auth(&auth).unwrap();
    assert_eq!(app.on_auth(&auth).unwrap_err(), AuthError::OutOfOrder);

    // The extension side refuses an unsolicited hello result.
    let mut ext = ExtHandshake::new(k, INSTANCE);
    assert_eq!(
        ext.on_hello_result(&result).unwrap_err(),
        AuthError::OutOfOrder
    );
}

#[test]
fn unpaired_extensions_get_e_not_paired_unless_a_window_is_open() {
    let hello = ExtHandshake::new(key(6), INSTANCE).hello(false, "1.0.0", "Chrome");
    let error = AppHandshake::new()
        .on_hello(&hello, &ctx(None, false), &mut ChallengeLog::default())
        .unwrap_err();
    assert_eq!(error, AuthError::NotPaired);
    assert_eq!(error.code(), ErrorCode::NotPaired);

    // Claims to be paired but the app has no key: also not paired.
    let hello = ExtHandshake::new(key(6), INSTANCE).hello(true, "1.0.0", "Chrome");
    let error = AppHandshake::new()
        .on_hello(&hello, &ctx(None, true), &mut ChallengeLog::default())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::NotPaired);
}

#[test]
fn protocol_mismatch_is_e_protocol_and_lists_the_supported_range() {
    let mut hello = ExtHandshake::new(key(7), INSTANCE).hello(true, "2.0.0", "Chrome");
    hello.protocol = [2, 3];
    let error = AppHandshake::new()
        .on_hello(
            &hello,
            &ctx(Some(&key(7)), false),
            &mut ChallengeLog::default(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Protocol);
    assert_eq!(
        error.to_protocol_error().detail.unwrap(),
        json!({"supported": [1, 1]})
    );
}

#[test]
fn bad_challenge_encodings_are_invalid() {
    for text in [
        "",
        "AAAA",
        &encode_bytes(&[0u8; 31]),
        &encode_bytes(&[0u8; 33]),
        "!!!!",
    ] {
        assert_eq!(
            error_of(decode_challenge(text)),
            ErrorCode::Invalid,
            "{text}"
        );
    }
    let mut hello = hello_params();
    hello.challenge = "short".into();
    let params = Some(serde_json::to_value(hello).unwrap());
    assert_eq!(error_of(HelloParams::parse(&params)), ErrorCode::Invalid);
}

fn sealed_pair() -> (Session, Session, Envelope) {
    let (mut app, ext) = connect(&key(8));
    let mut frame = Envelope::request("a-1", Method::PageRead, json!({"tab": 1, "max_bytes": 100}))
        .with_mono(MONO);
    app.seal(&mut frame).unwrap();
    (app, ext, frame)
}

#[test]
fn a_tampered_frame_fails_the_mac_and_closes_nothing_silently() {
    let (_, mut ext, frame) = sealed_pair();
    let mut tampered = frame.clone();
    tampered.params = Some(json!({"tab": 2, "max_bytes": 100}));
    assert_eq!(ext.open(&tampered).unwrap_err(), AuthError::BadFrameMac);
    let mut other_mono = frame.clone();
    other_mono.mono = Some("ffffffffffffffffffffffffffffffff".into());
    assert_eq!(ext.open(&other_mono).unwrap_err(), AuthError::BadFrameMac);
    let mut other_method = frame.clone();
    other_method.method = Some("page.find".into());
    assert_eq!(ext.open(&other_method).unwrap_err(), AuthError::BadFrameMac);
    let mut other_id = frame.clone();
    other_id.id = "a-2".into();
    assert_eq!(ext.open(&other_id).unwrap_err(), AuthError::BadFrameMac);
    let mut flipped = frame.clone();
    let mut mac = flipped.mac.clone().unwrap().into_bytes();
    mac[0] = if mac[0] == b'A' { b'B' } else { b'A' };
    flipped.mac = Some(String::from_utf8(mac).unwrap());
    assert_eq!(ext.open(&flipped).unwrap_err(), AuthError::BadFrameMac);
    let mut garbage = frame.clone();
    garbage.mac = Some("not base64!".into());
    assert_eq!(ext.open(&garbage).unwrap_err(), AuthError::BadFrameMac);
    // Failed attempts did not advance the counter: the genuine frame still opens.
    ext.open(&frame).unwrap();
}

#[test]
fn a_replayed_or_reordered_frame_is_refused() {
    let (mut app, mut ext, first) = sealed_pair();
    ext.open(&first).unwrap();
    assert_eq!(ext.open(&first).unwrap_err(), AuthError::Replay);
    let mut second = Envelope::request("a-2", Method::StatusGet, json!({}));
    app.seal(&mut second).unwrap();
    let mut third = Envelope::request("a-3", Method::StatusGet, json!({}));
    app.seal(&mut third).unwrap();
    ext.open(&third).unwrap();
    // Delivered late: seq 2 is now below the last accepted seq 3.
    assert_eq!(ext.open(&second).unwrap_err(), AuthError::Replay);
}

#[test]
fn frames_without_seq_or_mac_are_refused() {
    let (_, mut ext, frame) = sealed_pair();
    let mut no_mac = frame.clone();
    no_mac.mac = None;
    assert_eq!(ext.open(&no_mac).unwrap_err(), AuthError::Replay);
    let mut no_seq = frame.clone();
    no_seq.seq = None;
    assert_eq!(ext.open(&no_seq).unwrap_err(), AuthError::Replay);
    // A forged seq with the genuine mac does not verify.
    let mut bumped = frame;
    bumped.seq = Some(9);
    assert_eq!(ext.open(&bumped).unwrap_err(), AuthError::BadFrameMac);
}

#[test]
fn a_frame_cannot_be_reflected_back_to_its_sender() {
    let (mut app, _, frame) = sealed_pair();
    // The app's own frame fed back to the app fails: the direction byte differs.
    assert_eq!(app.open(&frame).unwrap_err(), AuthError::BadFrameMac);
}

#[test]
fn frames_from_another_session_do_not_verify() {
    let (mut app_a, _) = connect(&key(9));
    let (_, mut ext_b) = connect(&key(9));
    let mut frame = Envelope::request("a-1", Method::StatusGet, json!({}));
    app_a.seal(&mut frame).unwrap();
    // Same long-term key, different challenges: a different session key.
    assert_eq!(ext_b.open(&frame).unwrap_err(), AuthError::BadFrameMac);
}

#[test]
fn canonical_json_is_stable_and_integer_only() {
    let value = json!({"b": [1, true, null, "x\"y\n"], "a": {"z": 1, "y": -2}, "é": "ü\u{1f600}"});
    assert_eq!(
        canonical_json(&value).unwrap(),
        "{\"a\":{\"y\":-2,\"z\":1},\"b\":[1,true,null,\"x\\\"y\\n\"],\"é\":\"ü\u{1f600}\"}"
    );
    let from_text: Value = serde_json::from_str("{\"n\": 1.0, \"m\": 12}").unwrap();
    assert_eq!(canonical_json(&from_text).unwrap(), "{\"m\":12,\"n\":1}");
    for bad in ["1.5", "9007199254740992", "-9007199254740992", "1e300"] {
        let value: Value = serde_json::from_str(&format!("{{\"n\":{bad}}}")).unwrap();
        assert_eq!(
            error_of(canonical_json(&value)),
            ErrorCode::Invalid,
            "{bad}"
        );
    }
    let safe: Value = serde_json::from_str("{\"n\":9007199254740991}").unwrap();
    assert_eq!(canonical_json(&safe).unwrap(), "{\"n\":9007199254740991}");
}

#[test]
fn keys_never_print() {
    let k = key(10);
    assert!(!format!("{k:?}").contains("10"));
    let (app, _) = connect(&k);
    assert!(format!("{app:?}").contains("redacted"));
}

// ---- pairing ----

fn paired_hello() -> HelloParams {
    ExtHandshake::new(key(11), INSTANCE).hello(false, "1.0.0", "Chrome")
}

#[test]
fn pairing_hands_out_a_key_once_and_the_normal_handshake_then_works() {
    let (mut window, nonce) = PairingWindow::open(1_000);
    assert!(window.is_open(1_000));
    let mut app = AppHandshake::new();
    let mut log = ChallengeLog::default();
    let result = app
        .on_hello(&paired_hello(), &ctx(None, true), &mut log)
        .unwrap();
    assert!(result.mac.is_none());
    let params = PairBeginParams {
        nonce,
        instance_id: INSTANCE.into(),
    };
    let (reply, stored) = app.on_pair_begin(&params, &mut window, 2_000).unwrap();
    assert_eq!(
        PairingKey::from_b64(&reply.key).unwrap().as_bytes(),
        stored.as_bytes()
    );
    assert!(!window.is_open(2_000));

    // Later: the extension reconnects with the key it was given.
    let (mut app_session, mut ext_session) = connect(&stored);
    let mut frame = Envelope::request("a-1", Method::StatusGet, json!({}));
    app_session.seal(&mut frame).unwrap();
    ext_session.open(&frame).unwrap();
}

#[test]
fn a_pairing_nonce_works_once() {
    let (mut window, nonce) = PairingWindow::open_with(0, [5u8; NONCE_LEN]);
    window.consume(&nonce, 10).unwrap();
    let error = window.consume(&nonce, 11).unwrap_err();
    assert_eq!(error, AuthError::PairingNonceUsed);
    assert_eq!(error.code(), ErrorCode::NotPaired);

    // Reuse through a second connection is also refused.
    let mut app = AppHandshake::new();
    app.on_hello(
        &paired_hello(),
        &ctx(None, true),
        &mut ChallengeLog::default(),
    )
    .unwrap();
    let params = PairBeginParams {
        nonce,
        instance_id: INSTANCE.into(),
    };
    assert_eq!(
        app.on_pair_begin(&params, &mut window, 12).unwrap_err(),
        AuthError::PairingNonceUsed
    );
}

#[test]
fn a_pairing_nonce_expires_after_five_minutes() {
    let (mut window, nonce) = PairingWindow::open(1_000);
    assert!(window.is_open(1_000 + PAIRING_TTL_MS - 1));
    assert!(!window.is_open(1_000 + PAIRING_TTL_MS));
    assert_eq!(
        window.consume(&nonce, 1_000 + PAIRING_TTL_MS).unwrap_err(),
        AuthError::PairingExpired
    );
    // One millisecond before the deadline a correct nonce still works.
    let (mut window, nonce) = PairingWindow::open(0);
    window.consume(&nonce, PAIRING_TTL_MS - 1).unwrap();
}

#[test]
fn wrong_nonces_are_counted_and_close_the_window() {
    let (mut window, nonce) = PairingWindow::open_with(0, [6u8; NONCE_LEN]);
    for _ in 0..PAIRING_MAX_ATTEMPTS {
        let wrong = encode_bytes(&[7u8; NONCE_LEN]);
        assert_eq!(
            window.consume(&wrong, 1).unwrap_err(),
            AuthError::PairingBadNonce
        );
    }
    assert!(!window.is_open(1));
    assert_eq!(
        window.consume(&nonce, 2).unwrap_err(),
        AuthError::PairingClosed
    );

    let (mut window, _) = PairingWindow::open(0);
    for bad in [
        "",
        "not base64",
        &encode_bytes(&[1u8; 8]),
        &encode_bytes(&[1u8; 32]),
    ] {
        assert_eq!(
            window.consume(bad, 1).unwrap_err(),
            AuthError::PairingBadNonce
        );
    }
}

#[test]
fn pair_begin_needs_a_pairing_hello_and_the_same_instance() {
    let (mut window, nonce) = PairingWindow::open(0);
    let params = PairBeginParams {
        nonce: nonce.clone(),
        instance_id: INSTANCE.into(),
    };
    // Without a hello first.
    assert_eq!(
        AppHandshake::new()
            .on_pair_begin(&params, &mut window, 1)
            .unwrap_err(),
        AuthError::OutOfOrder
    );
    // The hello came from another instance.
    let mut app = AppHandshake::new();
    app.on_hello(
        &paired_hello(),
        &ctx(None, true),
        &mut ChallengeLog::default(),
    )
    .unwrap();
    let other = PairBeginParams {
        nonce,
        instance_id: "11111111111111111111111111111111".into(),
    };
    assert!(app.on_pair_begin(&other, &mut window, 1).is_err());
    assert!(
        window.is_open(1),
        "a mismatched instance must not spend the nonce"
    );
}

// ---- shared vectors ----

fn vector_key() -> PairingKey {
    PairingKey::from_bytes(std::array::from_fn(|i| i as u8))
}
fn vector_ext_challenge() -> Challenge {
    std::array::from_fn(|i| 0x40 + i as u8)
}
fn vector_app_challenge() -> Challenge {
    std::array::from_fn(|i| 0x80 + i as u8)
}

fn vector_frames() -> Vec<(Direction, Envelope)> {
    let mut one = Envelope::request(
        "a-1",
        Method::PageSnapshot,
        json!({"tab": 7, "mode": "full"}),
    )
    .with_mono(MONO);
    one.seq = Some(1);
    let mut two = Envelope::response_ok(
        "a-1",
        json!({"text": "- \"quoted\"\n\ttab \u{e9}\u{1f600}", "truncated": false, "generation": 3}),
    );
    two.seq = Some(1);
    let mut three = Envelope::event(
        "x-12",
        Event::TabOwnership,
        json!({"tab": 4, "owner": null, "by": "popup"}),
    );
    three.seq = Some(2);
    let mut four = Envelope::request(
        "a-9",
        Method::PageWait,
        json!({"tab": 1, "time_ms": 9007199254740991i64, "load": true}),
    )
    .with_mono(MONO);
    four.seq = Some(9007199254740990);
    vec![
        (Direction::AppToExtension, one),
        (Direction::ExtensionToApp, two),
        (Direction::ExtensionToApp, three),
        (Direction::AppToExtension, four),
    ]
}

fn mac_for_vector(direction: Direction, envelope: &Envelope) -> (String, String) {
    let ext_chal = vector_ext_challenge();
    let app_chal = vector_app_challenge();
    let input = frame_mac_input(direction, envelope).unwrap();
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(
        session_key(&vector_key(), &ext_chal, &app_chal).as_bytes(),
    )
    .unwrap();
    mac.update(&input);
    let canonical = {
        let mut unsigned = envelope.clone();
        unsigned.mac = None;
        canonical_json(&serde_json::to_value(unsigned).unwrap()).unwrap()
    };
    (canonical, encode_bytes(&mac.finalize().into_bytes()))
}

fn build_vectors() -> Value {
    let k = vector_key();
    let ext_chal = vector_ext_challenge();
    let app_chal = vector_app_challenge();
    let session = session_key(&k, &ext_chal, &app_chal);
    let wrong = PairingKey::from_bytes([0xaa; KEY_LEN]);
    let frames: Vec<Value> = vector_frames()
        .into_iter()
        .map(|(direction, envelope)| {
            let (canonical, mac) = mac_for_vector(direction, &envelope);
            json!({
                "direction": direction as u8,
                "envelope": envelope,
                "canonical": canonical,
                "mac": mac,
            })
        })
        .collect();
    json!({
        "description": "Test vectors for the zptah Browser link handshake and frame MAC (protocol 1). Generated by the Rust tests in crates/monocode-core/src/mono/browser/link/tests.rs; regenerate with ZPTAH_WRITE_VECTORS=1 cargo ntest -p monocode-core write_handshake_vectors -- --ignored. Binary values are standard base64 with padding.",
        "key": k.to_b64(),
        "instance_id": INSTANCE,
        "ext_challenge": encode_bytes(&ext_chal),
        "app_challenge": encode_bytes(&app_chal),
        "app_mac": encode_bytes(&app_mac(&k, &ext_chal, &app_chal, INSTANCE)),
        "ext_mac": encode_bytes(&ext_mac(&k, &ext_chal, &app_chal, INSTANCE)),
        "session_key": encode_bytes(session.as_bytes()),
        "wrong_key": wrong.to_b64(),
        "wrong_key_app_mac": encode_bytes(&app_mac(&wrong, &ext_chal, &app_chal, INSTANCE)),
        "formulas": {
            "app_mac": "HMAC-SHA256(key, \"zptah-browser/1|app|\" || ext_challenge || app_challenge || utf8(instance_id))",
            "ext_mac": "HMAC-SHA256(key, \"zptah-browser/1|ext|\" || app_challenge || ext_challenge || utf8(instance_id))",
            "session_key": "HKDF-SHA256(ikm = key, salt = ext_challenge || app_challenge, info = \"zptah-browser/1 session\", length 32)",
            "frame_mac": "HMAC-SHA256(session_key, direction_byte || seq_u64_big_endian || utf8(canonical)); direction 1 = extension to app, 2 = app to extension",
            "canonical": "envelope without mac, object keys sorted by code point, no whitespace, integers only (safe range), JSON string escaping as JSON.stringify"
        },
        "frames": frames,
        "canonical_cases": [
            {"json": "{\"b\":1,\"a\":{\"d\":[1,2],\"c\":null}}", "canonical": "{\"a\":{\"c\":null,\"d\":[1,2]},\"b\":1}"},
            {"json": "{\"n\": 1.0}", "canonical": "{\"n\":1}"},
            {"json": "{\"s\":\"\\u00e9\\n\\\"\"}", "canonical": "{\"s\":\"\u{e9}\\n\\\"\"}"}
        ],
        "canonical_rejects": ["{\"n\":1.5}", "{\"n\":9007199254740992}", "{\"n\":1e300}"]
    })
}

#[test]
#[ignore = "writes shared/handshake-vectors.json; set ZPTAH_WRITE_VECTORS=1"]
fn write_handshake_vectors() {
    if std::env::var("ZPTAH_WRITE_VECTORS").is_err() {
        return;
    }
    let text = serde_json::to_string_pretty(&build_vectors()).unwrap() + "\n";
    std::fs::write(VECTORS_PATH, text).unwrap();
}

#[test]
fn shared_vectors_match_the_implementation() {
    let committed: Value = serde_json::from_str(VECTORS_TEXT).expect("vectors file is JSON");
    assert_eq!(
        committed,
        build_vectors(),
        "regenerate handshake-vectors.json"
    );

    // Also verify the committed file through the public API, not only through the builder.
    let k = PairingKey::from_b64(committed["key"].as_str().unwrap()).unwrap();
    let ext_chal = decode_challenge(committed["ext_challenge"].as_str().unwrap()).unwrap();
    let app_chal = decode_challenge(committed["app_challenge"].as_str().unwrap()).unwrap();
    let id = committed["instance_id"].as_str().unwrap();
    let mut ext = ExtHandshake::with_challenge(k.clone(), id, ext_chal);
    let hello = ext.hello(true, "1.0.0", "Chrome");
    let mut app = AppHandshake::new();
    let result = app
        .on_hello_with(
            &hello,
            &ctx(Some(&k), false),
            &mut ChallengeLog::default(),
            app_chal,
        )
        .unwrap();
    assert_eq!(result.mac.as_deref(), committed["app_mac"].as_str());
    let auth = ext.on_hello_result(&result).unwrap();
    assert_eq!(Some(auth.mac.as_str()), committed["ext_mac"].as_str());
    let wrong_key = PairingKey::from_b64(committed["wrong_key"].as_str().unwrap()).unwrap();
    assert_ne!(
        encode_bytes(&app_mac(&wrong_key, &ext_chal, &app_chal, id)),
        committed["app_mac"].as_str().unwrap()
    );

    // Opening each committed frame with the matching side of a real session.
    let mut app_session = app.on_auth(&auth).unwrap();
    let mut ext_session = ext.into_session().unwrap();
    for frame in committed["frames"].as_array().unwrap() {
        let mut envelope: Envelope = serde_json::from_value(frame["envelope"].clone()).unwrap();
        envelope.mac = Some(frame["mac"].as_str().unwrap().to_string());
        let receiver = if frame["direction"] == 1 {
            &mut app_session
        } else {
            &mut ext_session
        };
        receiver.open(&envelope).unwrap();
    }
    for case in committed["canonical_cases"].as_array().unwrap() {
        let value: Value = serde_json::from_str(case["json"].as_str().unwrap()).unwrap();
        assert_eq!(
            canonical_json(&value).unwrap(),
            case["canonical"].as_str().unwrap()
        );
    }
    for text in committed["canonical_rejects"].as_array().unwrap() {
        let value: Value = serde_json::from_str(text.as_str().unwrap()).unwrap();
        assert!(canonical_json(&value).is_err());
    }
}
