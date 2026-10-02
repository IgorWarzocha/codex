//! Checks compact successes and opt-in timing without altering content or state.

use super::*;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

#[test]
fn nonempty_success_preserves_content_without_bookkeeping_for_exec_and_wait() {
    for content in [
        vec![FunctionCallOutputContentItem::InputText {
            text: "result\n[Output truncated]".to_string(),
        }],
        vec![FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "data:image/png;base64,image".to_string(),
            },
            detail: None,
        }],
    ] {
        let expected = FunctionToolOutput::from_content(content, Some(true));
        let mut output = CodeModeToolOutput::new(
            FunctionToolOutput::from_content(expected.body.clone(), Some(true)),
            "Script completed".to_string(),
            Duration::from_millis(750),
            None,
        );
        output.set_handler_duration_ms(1250);
        for payload in [
            ToolPayload::Custom {
                input: "text('result')".to_string(),
            },
            ToolPayload::Function {
                arguments: r#"{"cell_id":"1"}"#.to_string(),
            },
        ] {
            assert_eq!(
                output.to_response_item("call", &payload),
                expected.to_response_item("call", &payload)
            );
        }
        assert!(output.success_for_logging());
    }
}

#[test]
fn empty_success_and_nonterminal_states_keep_their_acknowledgement() {
    for (status, content) in [
        ("Script completed", vec![]),
        (
            "Script completed",
            vec![FunctionCallOutputContentItem::InputText {
                text: String::new(),
            }],
        ),
        ("Script running with cell ID 17", vec![]),
        ("Script terminated", vec![]),
    ] {
        let output = CodeModeToolOutput::new(
            FunctionToolOutput::from_content(content, Some(true)),
            status.to_string(),
            Duration::ZERO,
            None,
        );
        assert_eq!(
            output.output.body[0],
            FunctionCallOutputContentItem::InputText {
                text: format!("{status}\n"),
            }
        );
    }
}

#[test]
fn completed_timing_preserves_content_and_distinguishes_zero_from_unavailable() {
    let content = vec![
        FunctionCallOutputContentItem::InputText {
            text: "result".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "data:image/png;base64,image".to_string(),
            },
            detail: None,
        },
        FunctionCallOutputContentItem::InputAudio {
            audio_url: "data:audio/wav;base64,audio".to_string(),
        },
    ];
    for (host_duration, expected_timing) in [
        (
            Some(Duration::from_millis(/*millis*/ 750)),
            "Wall time 1.250 seconds (code-mode 0.750 seconds; overhead 0.500 seconds)",
        ),
        (
            Some(Duration::ZERO),
            "Wall time 1.250 seconds (code-mode 0.000 seconds; overhead 1.250 seconds)",
        ),
        (
            Some(Duration::from_micros(/*micros*/ 1_251_400)),
            "Wall time 1.250 seconds (code-mode 1.251 seconds; overhead -0.001 seconds)",
        ),
        (None, ""),
    ] {
        let mut output = CodeModeToolOutput::new(
            FunctionToolOutput::from_content(content.clone(), Some(false)),
            "Script failed".to_string(),
            Duration::from_millis(/*millis*/ 750),
            host_duration,
        );
        output.set_handler_duration_ms(/*handler_duration_ms*/ 1_250);
        let mut expected_content = vec![FunctionCallOutputContentItem::InputText {
            text: if host_duration.is_some() {
                format!("Script failed\n{expected_timing}\nOutput:\n")
            } else {
                "Script failed\n".to_string()
            },
        }];
        expected_content.extend(content.clone());
        let expected = FunctionToolOutput::from_content(expected_content, Some(false));
        for payload in [
            ToolPayload::Custom {
                input: "text('result')".to_string(),
            },
            ToolPayload::Function {
                arguments: r#"{"cell_id":"1"}"#.to_string(),
            },
        ] {
            let response = output.to_response_item("call", &payload);
            assert_eq!(response, expected.to_response_item("call", &payload));
            // Serialization must reuse the completed value, including on retries.
            assert_eq!(output.to_response_item("call", &payload), response);
        }
        assert_eq!(output.success_for_logging(), false);
    }
}
