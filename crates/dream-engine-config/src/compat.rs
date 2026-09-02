// Configuration-driven provider compatibility layer.
// Each provider type has default presets; users can override any field via config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use dream_engine_types::message::ImageInputCapability;

/// Provider-level compatibility settings.
/// Each child struct is flattened so on-disk TOML remains backward-compatible.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderCompat {
    #[serde(flatten)]
    pub transport: TransportCompat,
    #[serde(flatten)]
    pub messages: MessageCompat,
    #[serde(flatten)]
    pub tools: ToolCompat,
    #[serde(flatten)]
    pub schema: SchemaCompat,
    #[serde(flatten)]
    pub reasoning: ReasoningCompat,
    /// Image-input support resolved for the concrete provider/model pair.
    ///
    /// `None` is treated as `Unknown`; provider presets intentionally do not
    /// supply family-level defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_input: Option<ImageInputCapability>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TransportCompat {
    /// OpenAI wire API used for request and response projection.
    /// Default: chat_completions for backward compatibility.
    pub openai_api_mode: Option<OpenAiApiMode>,

    /// Field name for max tokens in request body.
    /// Default: "max_tokens" for all providers.
    pub max_tokens_field: Option<String>,

    /// Default max_tokens when the request does not set one.
    /// Default: provider-specific. None means omit the field if unset.
    pub default_max_tokens: Option<u32>,

    /// Model substring rules for default max_tokens.
    /// The first matching pattern wins.
    pub model_max_tokens: Option<Vec<ModelMaxTokensRule>>,

    /// Custom API path appended to base_url for chat completions.
    /// Default: "/chat/completions" for OpenAI-compatible providers.
    pub api_path: Option<String>,

    /// Maximum serialized provider request body size in bytes.
    /// Default: None (no local preflight limit).
    pub max_request_body_bytes: Option<usize>,

    /// Whether OpenAI-compatible requests include stream_options.
    /// Default: true for OpenAI-compatible providers.
    pub include_stream_options: Option<bool>,

    /// Ollama `options.num_ctx` — the server-side context window the daemon
    /// allocates for the model. Only read by the Ollama native transport.
    /// `None` means "do not send num_ctx": the daemon then keeps its own
    /// default (4096 unless OLLAMA_CONTEXT_LENGTH says otherwise), which is
    /// the safe choice because an over-large num_ctx makes Ollama allocate
    /// KV cache the machine may not have.
    pub num_ctx: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct ModelMaxTokensRule {
    pub pattern: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MessageCompat {
    /// Merge consecutive assistant messages (text concat + tool_calls merge).
    /// Default: true for openai.
    pub merge_assistant_messages: Option<bool>,

    /// Remove tool_result blocks that have no corresponding tool_use.
    /// Default: true for provider families that support tool results.
    pub clean_orphan_tool_results: Option<bool>,

    /// Deduplicate tool results with same tool_call_id (keep last).
    /// Default: true for openai.
    pub dedup_tool_results: Option<bool>,

    /// Ensure messages alternate user/assistant (insert filler if needed).
    /// Default: true for anthropic/bedrock/vertex.
    pub ensure_alternation: Option<bool>,

    /// Merge consecutive same-role messages into one.
    /// Default: true for anthropic/bedrock/vertex.
    pub merge_same_role: Option<bool>,

    /// Text patterns to strip from message history before sending.
    /// Default: empty.
    pub strip_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ToolCompat {
    /// Remove tool_use blocks that have no corresponding tool_result.
    /// Default: true for openai.
    pub clean_orphan_tool_calls: Option<bool>,

    /// Downgrade malformed tool_calls in the projected request body.
    /// Default: true for all providers.
    pub sanitize_malformed_tool_calls: Option<bool>,

    /// Auto-generate tool IDs when missing.
    /// Default: true for anthropic/bedrock/vertex.
    pub auto_tool_id: Option<bool>,

    /// Maximum number of tools allowed in the projected provider request.
    /// Default: None (no local preflight limit).
    pub max_tool_count: Option<usize>,

    /// Whether OpenAI-compatible requests include outgoing tools.
    /// Default: true for OpenAI-compatible providers.
    pub emit_tools: Option<bool>,

    /// Explicit tools declaration wire shape.
    /// Default: native provider path shape.
    pub tool_wire_shape: Option<ToolWireShape>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
pub enum ToolWireShape {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "openai_function")]
    OpenAiFunction,
    #[serde(rename = "anthropic_input_schema")]
    AnthropicInputSchema,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SchemaCompat {
    /// Sanitize JSON schemas for strict providers.
    /// Default: true for bedrock.
    pub sanitize_schema: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ReasoningCompat {
    /// Whether this provider supports extended thinking.
    /// Default: true for anthropic/bedrock/vertex, false for openai.
    pub supports_thinking: Option<bool>,

    /// Whether this provider supports reasoning_effort.
    /// Default: false for anthropic/bedrock/vertex, true for openai.
    pub supports_effort: Option<bool>,

    /// Available effort levels for this provider.
    /// Only meaningful when supports_effort is true.
    pub effort_levels: Option<Vec<String>>,

    /// Replay historical thinking as an Anthropic-style `content[]` array
    /// block (`{"type":"thinking","thinking":"..."}`) instead of the flat
    /// OpenAI/DeepSeek-style `reasoning_content` string field.
    ///
    /// Some OpenAI-protocol gateways front a thinking-capable model through
    /// an Anthropic-shaped validation layer and reject `reasoning_content`
    /// as "thinking not passed back" even though it *was* sent. Default:
    /// false (use `reasoning_content`) everywhere; the OpenAI transport
    /// flips this on for a single retry when it observes that exact
    /// rejection (see `composed.rs`).
    pub thinking_replay_as_content_block: Option<bool>,

    /// Omit historical thinking/reasoning content from the replayed request
    /// entirely (send neither `reasoning_content` nor a `content[].thinking`
    /// block). Last-resort fallback for gateways that reject *any* replay
    /// shape because they require a cryptographic signature the
    /// OpenAI-protocol streaming endpoint never hands the client — there is
    /// no shape a client can send that satisfies that gateway, so dropping
    /// the claim entirely is the only remaining option. Default: false.
    pub omit_thinking_replay: Option<bool>,

    /// Replay historical tool traffic as plain text: assistant `tool_calls`
    /// become bracketed text lines in `content`, and `tool` role results
    /// become `user` text messages. Implies omitting thinking replay.
    ///
    /// Black-box probing of a real gateway (LiteLLM-style, fronting
    /// DeepSeek V4 thinking models) showed it 400s on *any* request whose
    /// history contains an assistant `tool_calls` entry — regardless of
    /// thinking declaration or replay format — because its upstream
    /// conversion cannot round-trip thinking blocks alongside tool use.
    /// The same conversation replayed as plain text succeeds. Default:
    /// false; the OpenAI transport escalates to this as the final
    /// automatic retry (see `composed.rs`).
    pub textualize_tool_replay: Option<bool>,

    /// Model substring patterns for models that reason **by default** — a
    /// hybrid-reasoning model (GLM-4.5+/Z1, DeepSeek-R, …) fronted by an
    /// OpenAI-compatible gateway keeps emitting `reasoning_content` even though
    /// the caller never asked for thinking, and on an open-ended request it can
    /// spend an entire `max_tokens` budget reasoning without ever answering
    /// (observed: `glm-flash-latest` on a LiteLLM gateway, ~15k reasoning
    /// tokens / ~10 min / empty answer).
    ///
    /// When the caller did **not** set `request.thinking` and the model matches
    /// one of these, the OpenAI ChatCompletions projector sends
    /// `thinking: {"type": "disabled"}` to turn the model's built-in reasoning
    /// off. Matching mirrors [`Self::default_max_tokens_for_model`]
    /// (case-insensitive substring, `.`→`-`, first match wins). Empty / `None`
    /// = never auto-disable; the `thinking.type=enabled` opt-in is unaffected.
    ///
    /// Not applied to `openai_official_defaults` — the official o-series/gpt-5
    /// endpoint rejects an unknown `thinking` argument.
    pub thinking_off_by_default_models: Option<Vec<String>>,
}

impl TransportCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            openai_api_mode: user.openai_api_mode.or(defaults.openai_api_mode),
            max_tokens_field: user.max_tokens_field.or(defaults.max_tokens_field),
            default_max_tokens: user.default_max_tokens.or(defaults.default_max_tokens),
            model_max_tokens: user.model_max_tokens.or(defaults.model_max_tokens),
            api_path: user.api_path.or(defaults.api_path),
            max_request_body_bytes: user.max_request_body_bytes.or(defaults.max_request_body_bytes),
            include_stream_options: user.include_stream_options.or(defaults.include_stream_options),
            num_ctx: user.num_ctx.or(defaults.num_ctx),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiApiMode {
    #[default]
    ChatCompletions,
    Responses,
}

impl MessageCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            merge_assistant_messages: user.merge_assistant_messages.or(defaults.merge_assistant_messages),
            clean_orphan_tool_results: user.clean_orphan_tool_results.or(defaults.clean_orphan_tool_results),
            dedup_tool_results: user.dedup_tool_results.or(defaults.dedup_tool_results),
            ensure_alternation: user.ensure_alternation.or(defaults.ensure_alternation),
            merge_same_role: user.merge_same_role.or(defaults.merge_same_role),
            strip_patterns: user.strip_patterns.or(defaults.strip_patterns),
        }
    }
}

impl ToolCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            clean_orphan_tool_calls: user.clean_orphan_tool_calls.or(defaults.clean_orphan_tool_calls),
            sanitize_malformed_tool_calls: user
                .sanitize_malformed_tool_calls
                .or(defaults.sanitize_malformed_tool_calls),
            auto_tool_id: user.auto_tool_id.or(defaults.auto_tool_id),
            max_tool_count: user.max_tool_count.or(defaults.max_tool_count),
            emit_tools: user.emit_tools.or(defaults.emit_tools),
            tool_wire_shape: user.tool_wire_shape.or(defaults.tool_wire_shape),
        }
    }
}

impl SchemaCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            sanitize_schema: user.sanitize_schema.or(defaults.sanitize_schema),
        }
    }
}

impl ReasoningCompat {
    fn merge(defaults: Self, user: Self) -> Self {
        Self {
            supports_thinking: user.supports_thinking.or(defaults.supports_thinking),
            supports_effort: user.supports_effort.or(defaults.supports_effort),
            effort_levels: user.effort_levels.or(defaults.effort_levels),
            thinking_replay_as_content_block: user
                .thinking_replay_as_content_block
                .or(defaults.thinking_replay_as_content_block),
            omit_thinking_replay: user.omit_thinking_replay.or(defaults.omit_thinking_replay),
            textualize_tool_replay: user.textualize_tool_replay.or(defaults.textualize_tool_replay),
            thinking_off_by_default_models: user
                .thinking_off_by_default_models
                .or(defaults.thinking_off_by_default_models),
        }
    }
}

impl ProviderCompat {
    /// Defaults for Anthropic-family providers (Anthropic, Vertex)
    pub fn anthropic_defaults() -> Self {
        Self {
            transport: TransportCompat {
                default_max_tokens: Some(128_000),
                model_max_tokens: Some(anthropic_model_max_tokens_rules()),
                ..Default::default()
            },
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                clean_orphan_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                auto_tool_id: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                ..Default::default()
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(false),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Defaults for Bedrock (Anthropic + schema sanitization)
    pub fn bedrock_defaults() -> Self {
        Self {
            transport: TransportCompat {
                default_max_tokens: Some(128_000),
                model_max_tokens: Some(anthropic_model_max_tokens_rules()),
                ..Default::default()
            },
            messages: MessageCompat {
                ensure_alternation: Some(true),
                merge_same_role: Some(true),
                clean_orphan_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                auto_tool_id: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                ..Default::default()
            },
            schema: SchemaCompat {
                sanitize_schema: Some(true),
            },
            reasoning: ReasoningCompat {
                supports_thinking: Some(true),
                supports_effort: Some(false),
                ..Default::default()
            },
            image_input: None,
        }
    }

    /// Defaults for OpenAI-compatible providers
    pub fn openai_defaults() -> Self {
        Self {
            transport: TransportCompat {
                openai_api_mode: Some(OpenAiApiMode::ChatCompletions),
                max_tokens_field: Some("max_tokens".into()),
                api_path: Some("/chat/completions".into()),
                include_stream_options: Some(true),
                // Without an explicit value the field is omitted from the
                // request entirely and the upstream gateway falls back to its
                // own default, which is often far lower than what the model
                // actually supports (observed: 4096 completion tokens on a
                // LiteLLM-fronted gateway) — long tool-call output (e.g. a
                // large `Write` call) gets cut off mid-argument well before
                // the model was done. A generous provider-family default
                // avoids that for the common case without hardcoding a
                // specific gateway or model.
                default_max_tokens: Some(32_000),
                ..Default::default()
            },
            messages: MessageCompat {
                merge_assistant_messages: Some(true),
                clean_orphan_tool_results: Some(true),
                dedup_tool_results: Some(true),
                ..Default::default()
            },
            tools: ToolCompat {
                clean_orphan_tool_calls: Some(true),
                sanitize_malformed_tool_calls: Some(true),
                auto_tool_id: Some(true),
                emit_tools: Some(true),
                ..Default::default()
            },
            reasoning: ReasoningCompat {
                // `supports_thinking` is a capability-exposure flag for the
                // host UI only; `thinking.type=enabled` stays opt-in and is
                // sent solely when the caller sets `request.thinking` (see
                // projector.rs, aligned with upstream #203).
                supports_thinking: Some(false),
                supports_effort: Some(true),
                effort_levels: Some(vec!["low".into(), "medium".into(), "high".into()]),
                // Hybrid-reasoning families that reason by default. Left off:
                // `deepseek-v3.1` / Qwen3 (thinking default varies by
                // deployment) and any bare `glm-4` (matches the non-reasoning
                // `glm-4-flash`). A user whose gateway serves another
                // reason-by-default model adds a pattern via provider compat.
                thinking_off_by_default_models: Some(vec![
                    "glm-flash-latest".into(),
                    "glm-latest".into(),
                    "glm-4-5".into(),
                    "glm-4-6".into(),
                    "glm-5".into(),
                    "glm-6".into(),
                    "glm-z1".into(),
                    "deepseek-r1".into(),
                    "deepseek-reasoner".into(),
                    "qwq".into(),
                ]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Defaults for the official OpenAI API (`api.openai.com`).
    ///
    /// Newer official models (o-series, gpt-5 family) reject the legacy
    /// `max_tokens` parameter and require `max_completion_tokens`, which all
    /// current official models accept. Third-party OpenAI-compatible
    /// endpoints often support only `max_tokens`, so plain
    /// [`openai_defaults`](Self::openai_defaults) keeps the legacy field and
    /// this preset applies only to the official endpoint.
    pub fn openai_official_defaults() -> Self {
        let mut compat = Self::openai_defaults();
        compat.transport.max_tokens_field = Some("max_completion_tokens".into());
        // The official endpoint rejects an unrecognized `thinking` argument,
        // and its reasoning models are driven by `reasoning_effort` instead.
        compat.reasoning.thinking_off_by_default_models = None;
        compat
    }

    /// Defaults for a local Ollama daemon speaking the native `/api/chat`
    /// protocol. Message/tool projection is OpenAI-shaped (Ollama accepts
    /// that shape), but there is no `stream_options`, no max-tokens field
    /// (the native transport maps it to `options.num_predict`), and no
    /// server-side default output cap — Ollama's own default is unlimited.
    pub fn ollama_defaults() -> Self {
        let mut compat = Self::openai_defaults();
        compat.transport.include_stream_options = Some(false);
        compat.transport.default_max_tokens = None;
        compat.transport.api_path = None;
        compat.reasoning.supports_effort = Some(false);
        compat.reasoning.effort_levels = None;
        compat
    }

    /// Merge user config over defaults (user wins on non-None fields)
    pub fn merge(defaults: Self, user: Self) -> Self {
        Self {
            transport: TransportCompat::merge(defaults.transport, user.transport),
            messages: MessageCompat::merge(defaults.messages, user.messages),
            tools: ToolCompat::merge(defaults.tools, user.tools),
            schema: SchemaCompat::merge(defaults.schema, user.schema),
            reasoning: ReasoningCompat::merge(defaults.reasoning, user.reasoning),
            image_input: user.image_input.or(defaults.image_input),
        }
    }

    // --- Resolved accessors (Option<bool> → bool with false default) ---

    pub fn max_tokens_field(&self) -> &str {
        self.transport.max_tokens_field.as_deref().unwrap_or("max_tokens")
    }

    pub fn openai_api_mode(&self) -> OpenAiApiMode {
        self.transport.openai_api_mode.unwrap_or_default()
    }

    /// Resolve the OpenAI endpoint path for the selected wire API.
    ///
    /// The historical `/chat/completions` preset remains the default for Chat
    /// Completions. When Responses is selected, that inherited preset is
    /// replaced with `/responses`; non-default custom paths remain honored.
    pub fn openai_api_path(&self) -> &str {
        match self.openai_api_mode() {
            OpenAiApiMode::ChatCompletions => self.api_path(),
            OpenAiApiMode::Responses => match self.transport.api_path.as_deref() {
                Some(path) if path != "/chat/completions" => path,
                _ => "/responses",
            },
        }
    }

    pub fn image_input(&self) -> ImageInputCapability {
        self.image_input.unwrap_or_default()
    }

    pub fn default_max_tokens_for_model(&self, model: &str) -> Option<u32> {
        let normalized = normalize_model_pattern(model);
        self.transport
            .model_max_tokens
            .as_deref()
            .and_then(|rules| {
                rules.iter().find_map(|rule| {
                    let pattern = normalize_model_pattern(&rule.pattern);
                    normalized.contains(&pattern).then_some(rule.max_tokens)
                })
            })
            .or(self.transport.default_max_tokens)
    }

    pub fn max_request_body_bytes(&self) -> Option<usize> {
        self.transport.max_request_body_bytes
    }

    pub fn max_tool_count(&self) -> Option<usize> {
        self.tools.max_tool_count
    }

    pub fn include_stream_options(&self) -> bool {
        self.transport.include_stream_options.unwrap_or(true)
    }

    pub fn num_ctx(&self) -> Option<u32> {
        self.transport.num_ctx
    }

    pub fn emit_tools(&self) -> bool {
        self.tools.emit_tools.unwrap_or(true)
    }

    pub fn tool_wire_shape(&self) -> ToolWireShape {
        self.tools.tool_wire_shape.unwrap_or(ToolWireShape::Native)
    }

    pub fn merge_assistant_messages(&self) -> bool {
        self.messages.merge_assistant_messages.unwrap_or(false)
    }

    pub fn clean_orphan_tool_calls(&self) -> bool {
        self.tools.clean_orphan_tool_calls.unwrap_or(false)
    }

    pub fn clean_orphan_tool_results(&self) -> bool {
        self.messages.clean_orphan_tool_results.unwrap_or(false)
    }

    pub fn dedup_tool_results(&self) -> bool {
        self.messages.dedup_tool_results.unwrap_or(false)
    }

    pub fn sanitize_malformed_tool_calls(&self) -> bool {
        self.tools.sanitize_malformed_tool_calls.unwrap_or(false)
    }

    pub fn ensure_alternation(&self) -> bool {
        self.messages.ensure_alternation.unwrap_or(false)
    }

    pub fn merge_same_role(&self) -> bool {
        self.messages.merge_same_role.unwrap_or(false)
    }

    pub fn sanitize_schema(&self) -> bool {
        self.schema.sanitize_schema.unwrap_or(false)
    }

    pub fn auto_tool_id(&self) -> bool {
        self.tools.auto_tool_id.unwrap_or(false)
    }

    pub fn api_path(&self) -> &str {
        self.transport.api_path.as_deref().unwrap_or("/chat/completions")
    }

    pub fn supports_thinking(&self) -> bool {
        self.reasoning.supports_thinking.unwrap_or(false)
    }

    /// Whether `model` reasons by default and should be told to stop when the
    /// caller did not ask for thinking. See
    /// [`ReasoningCompat::thinking_off_by_default_models`]. Matching mirrors
    /// [`Self::default_max_tokens_for_model`].
    pub fn thinking_off_by_default_for_model(&self, model: &str) -> bool {
        let normalized = normalize_model_pattern(model);
        self.reasoning
            .thinking_off_by_default_models
            .as_deref()
            .is_some_and(|patterns| {
                patterns
                    .iter()
                    .any(|pattern| normalized.contains(&normalize_model_pattern(pattern)))
            })
    }

    pub fn supports_effort(&self) -> bool {
        self.reasoning.supports_effort.unwrap_or(false)
    }

    pub fn effort_levels(&self) -> &[String] {
        self.reasoning.effort_levels.as_deref().unwrap_or(&[])
    }

    pub fn thinking_replay_as_content_block(&self) -> bool {
        self.reasoning.thinking_replay_as_content_block.unwrap_or(false)
    }

    /// Return a copy of this compat with `thinking_replay_as_content_block`
    /// forced on. Used for the single automatic retry in `composed.rs`.
    pub fn with_thinking_replay_as_content_block(&self) -> Self {
        let mut next = self.clone();
        next.reasoning.thinking_replay_as_content_block = Some(true);
        next
    }

    pub fn omit_thinking_replay(&self) -> bool {
        self.reasoning.omit_thinking_replay.unwrap_or(false)
    }

    /// Return a copy of this compat with thinking replay omitted entirely.
    /// Used for the second automatic retry in `composed.rs`, after the
    /// content-block retry also fails.
    pub fn with_thinking_replay_omitted(&self) -> Self {
        let mut next = self.clone();
        next.reasoning.thinking_replay_as_content_block = Some(false);
        next.reasoning.omit_thinking_replay = Some(true);
        next
    }

    pub fn textualize_tool_replay(&self) -> bool {
        self.reasoning.textualize_tool_replay.unwrap_or(false)
    }

    /// Return a copy of this compat with tool replay textualized (and
    /// thinking replay omitted). Used for the final automatic retry in
    /// `composed.rs`.
    pub fn with_textualized_tool_replay(&self) -> Self {
        let mut next = self.with_thinking_replay_omitted();
        next.reasoning.textualize_tool_replay = Some(true);
        next
    }
}

fn normalize_model_pattern(value: &str) -> String {
    value.to_ascii_lowercase().replace('.', "-")
}

fn anthropic_model_max_tokens_rules() -> Vec<ModelMaxTokensRule> {
    [
        ("claude-fable", 128_000),
        ("claude-opus-4-8", 128_000),
        ("claude-opus-4-7", 128_000),
        ("claude-opus-4-6", 128_000),
        ("claude-sonnet-4-6", 128_000),
        ("claude-opus-4-5", 64_000),
        ("claude-sonnet-4-5", 64_000),
        ("claude-haiku-4-5", 64_000),
        ("claude-opus-4", 32_000),
        ("claude-sonnet-4", 64_000),
        ("claude-3-7-sonnet", 128_000),
        ("claude-3-5-sonnet", 8_192),
        ("claude-3-5-haiku", 8_192),
        ("claude-3-opus", 4_096),
        ("claude-3-sonnet", 4_096),
        ("claude-3-haiku", 4_096),
        ("minimax", 131_072),
        ("qwen3", 65_536),
    ]
    .into_iter()
    .map(|(pattern, max_tokens)| ModelMaxTokensRule {
        pattern: pattern.to_string(),
        max_tokens,
    })
    .collect()
}

/// Sanitize a JSON Schema for strict providers (e.g., Bedrock).
/// - Root type must be "object" (wrap if not)
/// - Recursively remove "additionalProperties"
/// - Normalize array types: ["string", "null"] → "string"
pub fn sanitize_json_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();

    // Ensure root type is "object"
    if schema.get("type").and_then(|t| t.as_str()) != Some("object") {
        schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": schema
            },
            "required": ["value"]
        });
    }

    strip_additional_properties(&mut schema);
    normalize_array_types(&mut schema);
    schema
}

fn strip_additional_properties(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        obj.remove("additionalProperties");
        for v in obj.values_mut() {
            strip_additional_properties(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            strip_additional_properties(v);
        }
    }
}

fn normalize_array_types(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        // Normalize ["string", "null"] → "string"
        if let Some(arr) = obj.get("type").and_then(Value::as_array) {
            let non_null: Vec<&Value> = arr.iter().filter(|v| v.as_str() != Some("null")).collect();
            if non_null.len() == 1 {
                obj.insert("type".to_string(), non_null[0].clone());
            }
        }
        for v in obj.values_mut() {
            normalize_array_types(v);
        }
    } else if let Some(arr) = val.as_array_mut() {
        for v in arr.iter_mut() {
            normalize_array_types(v);
        }
    }
}

#[cfg(test)]
#[path = "compat_test.rs"]
mod compat_test;
