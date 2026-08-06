//! Turning a [`WorkflowSummary`] into harness-readable skill text.
//!
//! Rendering is deliberately target-independent: one body serves every harness,
//! and [`super::targets`] decides only where it lands. The generated text has
//! one job — make a model that has never seen this workflow call
//! `mcp__medulla__workflow_run` with the right argument shape, and behave
//! sensibly when the tool is not attached.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tinyflows::model::{InputType, WorkflowInput};

use crate::workflows::WorkflowSummary;

/// The tool a generated skill instructs the model to call.
const RUN_TOOL: &str = "mcp__medulla__workflow_run";

/// The tool that answers what the started run did.
///
/// Named in the body because starting a run and reading it are now two calls: a
/// skill that only knew the first would report "it is running" and never say
/// how it went.
const GET_TOOL: &str = "mcp__medulla__workflow_run_get";

/// The longest slug either verified harness accepts as a skill name.
///
/// Codex rejects a name over 64 characters outright (`InvalidField`), and the
/// name it validates is the directory basename when the frontmatter omits one.
/// Truncating here rather than letting the harness refuse keeps the failure
/// visible in the path the operator can see, instead of in a startup warning
/// they will not read.
const MAX_SLUG_CHARS: usize = 64;

/// The namespace every generated slug opens with.
///
/// Load-bearing beyond the cosmetic: [`super::install`] treats a `medulla-`
/// skill directory with no workflow behind it as ours to retire, so the prefix
/// is the one thing that makes a marker-less leftover recognisable.
pub(crate) const SLUG_PREFIX: &str = "medulla-";

/// The slug a workflow's skill is installed under.
///
/// Prefixed with `medulla-` so a directory listing of `~/.claude/skills` says
/// where these came from, and sanitised to the `[a-z0-9-]` alphabet that both
/// verified harnesses accept as a skill name — Codex additionally tokenises
/// `$name` mentions over `[A-Za-z0-9_\-:]`, so a slug carrying a dot or a space
/// could never be mentioned at all. An id that sanitises to nothing (all
/// punctuation) still yields a usable, if opaque, `medulla-workflow`.
pub fn slug_for(workflow_id: &str) -> String {
    let mut slug = String::with_capacity(workflow_id.len() + 8);
    let mut pending_dash = false;
    for ch in workflow_id.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        slug.push_str("workflow");
    }
    let mut slug = format!("{SLUG_PREFIX}{slug}");
    if slug.len() > MAX_SLUG_CHARS {
        // ASCII by construction, so a byte truncation is a char truncation. A
        // trailing dash would read as an accident.
        slug.truncate(MAX_SLUG_CHARS);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    slug
}

/// Renders the `SKILL.md` for one workflow.
///
/// The returned [`RenderedSkill::body`] is the complete file: frontmatter whose
/// first entry is the managed marker, then the instructions. `rev` fingerprints
/// the generated content, so it changes if and only if that content does —
/// which is what makes an install idempotent across releases that do not touch
/// the template.
pub fn render(summary: &WorkflowSummary) -> super::RenderedSkill {
    let slug = slug_for(&summary.id);
    let description = description_for(summary);
    let content = skill_content(summary, &slug, &description);
    let (body, rev) = seal(&summary.id, &content);
    super::RenderedSkill {
        workflow_id: summary.id.clone(),
        slug,
        description,
        body,
        rev,
    }
}

/// Renders the slash-command variant of an already-rendered skill.
///
/// The command exists for the operator who would rather type `/medulla-babysit
/// 123` than describe the work; the model-facing instructions are the same call
/// with the typed text handed over as `$ARGUMENTS`. Returns the complete file,
/// marker included, so it follows the same managed-file discipline.
pub fn render_command(skill: &super::RenderedSkill, summary: &WorkflowSummary) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!(
        "description: {}\n",
        yaml_scalar(&skill.description)
    ));
    out.push_str(&format!(
        "argument-hint: {}\n",
        yaml_scalar(&argument_hint(&summary.inputs))
    ));
    out.push_str("---\n\n");
    out.push_str(&format!(
        "Run the `{id}` Medulla workflow for the operator.\n\n",
        id = summary.id
    ));
    out.push_str(&format!(
        "Call `{RUN_TOOL}` with:\n\n```json\n{example}\n```\n\n",
        example = call_example(summary)
    ));
    out.push_str(
        "The operator typed: $ARGUMENTS\n\n\
         Map that text onto the inputs above. Ask for any required input it does not \
         supply rather than guessing — a missing or misnamed input is rejected and \
         nothing runs.\n\n",
    );
    out.push_str(&inputs_section(&summary.inputs));
    out.push('\n');
    out.push_str(&format!(
        "The call comes back at once with a `runId`; the run keeps going without it, \
         and can take minutes to hours. Follow it with `{GET_TOOL}` using that id, and \
         report the run's status and, if it failed, the id and error of the failing \
         step.\n\n{}",
        fallback_section(summary)
    ));
    seal(&summary.id, &out).0
}

/// The marker layout a `rev` is computed for.
///
/// Mixed into the hash so that moving the marker — as release 0.8 moved it from
/// a line above the frontmatter to a comment inside it — changes every `rev`
/// even where the rendered text is byte-identical. Without it an install would
/// find its own matching `rev` on a file laid out the old way, report
/// `unchanged`, and leave the operator with skills no harness can read.
const MARKER_FORMAT: &str = "medulla-skill-marker/2";

/// Wraps rendered content in its managed marker and returns `(file, rev)`.
///
/// The marker is what every write path checks before touching a file, so it is
/// produced in exactly one place. It goes *inside* the frontmatter, as a YAML
/// comment on its first line, because a harness only recognises frontmatter
/// that opens on line 1: with the marker above it, Claude Code read the whole
/// document as body text and showed the marker itself where the description
/// belongs — which is the one field the model matches a request against, so the
/// skill was installed, listed, and untriggerable. YAML drops `#` comments
/// before anything else sees the document, so the marker stays invisible to the
/// harness and legible to us.
///
/// The id is percent-encoded, because the store accepts any id that is a single
/// path component — spaces, newlines, quotes and non-ASCII included — and the
/// marker is a whitespace-separated, single-line field list. An id written raw
/// would either come back as a different id or split the marker across lines,
/// and both make Medulla disown a file it wrote itself.
///
/// Content that does not open with frontmatter falls back to the marker on its
/// own first line. Nothing here generates such content, but a marker that is
/// silently dropped would make the file unrecognisable — and therefore neither
/// updatable nor removable.
fn seal(workflow_id: &str, content: &str) -> (String, String) {
    let rev = format!(
        "{:x}",
        Sha256::digest(format!("{MARKER_FORMAT}\n{content}").as_bytes())
    );
    let id = encode_marker_id(workflow_id);
    let marker = format!("medulla:managed workflow={id} rev={rev}");
    let file = match content.strip_prefix("---\n") {
        Some(rest) => format!("---\n# {marker}\n{rest}"),
        None => format!("<!-- {marker} -->\n{content}"),
    };
    (file, rev)
}

/// The characters a marker id keeps as itself.
///
/// Deliberately narrow — alphanumerics and the three punctuation marks a
/// workflow id normally uses. Everything else, including `%` itself, becomes an
/// escape, so encoding is injective and decoding needs no lookahead rules.
fn is_marker_literal(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

/// Percent-encodes a workflow id for the marker line.
///
/// Operates on UTF-8 bytes, so a non-ASCII id round-trips exactly rather than
/// being transliterated.
fn encode_marker_id(workflow_id: &str) -> String {
    let mut out = String::with_capacity(workflow_id.len());
    for byte in workflow_id.as_bytes() {
        if is_marker_literal(*byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Decodes a marker id, or `None` when the field is not something
/// [`encode_marker_id`] could have produced.
///
/// Malformed input is rejected rather than guessed at: a half-written escape or
/// a stray literal byte means the line is not a marker we wrote, and treating
/// it as one would let a foreign file be adopted — or overwritten.
fn decode_marker_id(field: &str) -> Option<String> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = field.get(index + 1..index + 3)?;
                if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte if is_marker_literal(byte) => {
                out.push(byte);
                index += 1;
            }
            _ => return None,
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    if decoded.is_empty() {
        return None;
    }
    Some(decoded)
}

/// Reads the marker out of a file's head.
///
/// Two spellings are accepted, and both must stay: the current one is a YAML
/// comment inside the leading frontmatter, and the legacy one is an HTML
/// comment on line 1, above it. A release that stopped recognising the legacy
/// form would disown every file the previous release installed — reporting a
/// collision against its own skill on reinstall, and refusing to remove it on
/// `sync --prune` or `uninstall`.
///
/// Returns the `(workflow id, rev)` the marker names, or `None` when the file
/// is not ours — which the install path treats as "leave it alone", never as
/// "assume it is stale". A marker whose fields are duplicated, unparseable, or
/// missing counts as not ours for the same reason: the only safe reading of a
/// line we cannot fully account for is that someone else wrote it.
///
/// The marker occupies an exact slot, not merely "somewhere inside the
/// frontmatter": [`seal`] always splices it onto the line immediately after
/// the opening `---`, so a well-formed file of ours has it on line 2 and
/// nowhere else. Accepting it anywhere else in the block would let a
/// hand-written skill whose own frontmatter happens to carry a
/// `# medulla:managed` comment — documentation about this feature, or a
/// migration note — be adopted, and later overwritten or pruned, as if we had
/// written it. A `---` that never closes is rejected outright for the same
/// reason: it is not frontmatter any parser would recognise, so nothing
/// inside it can be read as ours either.
///
/// "Exact slot" includes the indentation: the line is matched without being
/// left-trimmed first, because `seal` never indents it and YAML happily accepts
/// an indented comment. Trimming would adopt a hand-written
/// `  # medulla:managed …` on the strength of whitespace we could not have
/// produced. The legacy HTML form keeps its tolerance — files already on disk
/// were written before this rule existed.
pub(crate) fn parse_marker(file: &str) -> Option<(String, String)> {
    let mut lines = file.lines();
    let first = lines.next()?;
    if let Some(rest) = first.trim().strip_prefix("<!-- medulla:managed ") {
        return parse_marker_fields(rest.strip_suffix("-->")?.trim());
    }
    // The opener is held to the same column-zero rule as the marker below it,
    // and for the same reason: an indented `  ---` is not frontmatter any
    // parser opens, so a file starting with one is not a skill of ours however
    // its second line reads.
    if first.trim_end() != "---" {
        return None;
    }
    // Trailing whitespace only: an editor may strip or add it at end of line,
    // but a leading space is a different line from the one we write.
    let rest = lines.next()?.strip_prefix("# medulla:managed ")?;
    // The block still has to close for this to be frontmatter at all — a
    // marker-shaped second line above an unclosed `---` is not one either.
    if !lines.any(|line| line.trim_end() == "---") {
        return None;
    }
    parse_marker_fields(rest.trim())
}

/// Parses the marker's `key=value` field list, whatever comment wrapped it.
fn parse_marker_fields(rest: &str) -> Option<(String, String)> {
    let mut workflow = None;
    let mut rev = None;
    for field in rest.split_whitespace() {
        // Unknown keys are tolerated so a marker written by a later release
        // still identifies its workflow; a field that is not `key=value` at all
        // is not.
        let (key, value) = field.split_once('=')?;
        match key {
            "workflow" if workflow.is_some() => return None,
            "workflow" => workflow = Some(decode_marker_id(value)?),
            "rev" if rev.is_some() => return None,
            "rev" => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                rev = Some(value.to_string());
            }
            _ => {}
        }
    }
    Some((workflow?, rev?))
}

/// The frontmatter description: the workflow's own words plus an explicit
/// trigger clause.
///
/// The clause is not decoration. A description that only restates the workflow
/// name gives the model nothing to match a paraphrased request against, and the
/// whole feature turns on that match. It is also why the result is never empty:
/// a workflow whose author wrote no description still gets a usable trigger.
fn description_for(summary: &WorkflowSummary) -> String {
    let own = summary.description.trim();
    let trigger = format!(
        "Use when the operator asks to run the Medulla \"{id}\" workflow, or describes the work it does.",
        id = summary.id
    );
    if own.is_empty() {
        trigger
    } else {
        let own = if own.ends_with('.') || own.ends_with('!') || own.ends_with('?') {
            own.to_string()
        } else {
            format!("{own}.")
        };
        format!("{own} {trigger}")
    }
}

/// The whole skill body below the marker.
fn skill_content(summary: &WorkflowSummary, slug: &str, description: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {slug}\n"));
    out.push_str(&format!("description: {}\n", yaml_scalar(description)));
    out.push_str("---\n\n");

    out.push_str(&format!(
        "# Run the `{id}` Medulla workflow\n\n",
        id = summary.id
    ));
    if !summary.description.trim().is_empty() {
        out.push_str(&format!("{}\n\n", summary.description.trim()));
    }
    out.push_str(&format!(
        "Start it with the Medulla MCP server — one tool call, no shell:\n\n```json\n{example}\n```\n\n",
        example = call_example(summary)
    ));

    out.push_str(&inputs_section(&summary.inputs));
    out.push('\n');

    out.push_str(&format!(
        "## While it runs\n\n\
         This call **does not wait for the run**. It starts it and answers at once with \
         a `runId`, and the run carries on without the call — a workflow can take \
         minutes to hours. Do not start it again; tell the operator it is under way if \
         they ask, and follow the run by its id.\n\n\
         ## Reading the result\n\n\
         Call `{GET_TOOL}` with that `runId`. While `status` is `running` the run has \
         not finished — say so and check again later rather than treating it as a \
         result. Once it settles, `status` is `succeeded`, `failed`, or `cancelled`, and \
         `steps` lists each node with its own status and a bounded preview of its \
         output; pass `\"steps\": \"full\"` when a truncated one is what you need to \
         read. On failure, report the id of the first failing step and its error \
         verbatim rather than summarising it away; that string is what the operator \
         needs to fix the workflow.\n\n"
    ));
    out.push_str(&fallback_section(summary));
    out
}

/// The inputs table, or the sentence that replaces it when there are none.
fn inputs_section(inputs: &[WorkflowInput]) -> String {
    if inputs.is_empty() {
        return "## Inputs\n\nThis workflow takes no inputs. Pass `\"inputs\": {}`.\n".to_string();
    }
    let mut out = String::from("## Inputs\n\n| name | type | required | meaning | default |\n| --- | --- | --- | --- | --- |\n");
    for input in inputs {
        let meaning = input
            .description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("—");
        let default = match &input.default {
            Some(value) => format!("`{value}`"),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "| `{name}` | {ty} | {required} | {meaning} | {default} |\n",
            name = input.name,
            ty = input.ty.as_str(),
            required = if input.required { "yes" } else { "no" },
            meaning = escape_cell(meaning),
        ));
    }
    out.push_str(
        "\nCollect every required input from the operator before calling; do not invent \
         one. A missing or misnamed input is rejected and nothing runs.\n",
    );
    out
}

/// The paragraph that keeps a skill useful when the server is not attached.
///
/// Skills are copied into user-scope directories that outlive any particular
/// MCP configuration, so the tool being absent is a normal state, not a bug —
/// and a model that dead-ends there is worse than no skill at all.
fn fallback_section(summary: &WorkflowSummary) -> String {
    format!(
        "## If the tool is not available\n\n\
         `{RUN_TOOL}` missing means the Medulla MCP server is not attached to this \
         session. Do not claim the run started. Either run it from the shell:\n\n\
         ```sh\nmedulla workflow run {id} --inputs '{inputs}'\n```\n\n\
         or attach the server once with `medulla skills install --with-mcp` and try the \
         tool again.\n",
        id = shell_quote_arg(&summary.id),
        inputs = example_inputs(summary),
    )
}

/// A concrete, copyable call for this workflow's declared signature.
fn call_example(summary: &WorkflowSummary) -> String {
    let call = json!({ "id": summary.id, "inputs": example_input_map(summary) });
    format!(
        "{RUN_TOOL}\n{}",
        serde_json::to_string_pretty(&call).unwrap_or_else(|_| "{}".to_string())
    )
}

/// Just the inputs object, compact, for the shell fallback.
fn example_inputs(summary: &WorkflowSummary) -> String {
    serde_json::to_string(&Value::Object(example_input_map(summary)))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Placeholder values for every declared input.
///
/// An input with a default shows its default (so the example is runnable as
/// written); everything else shows a type-shaped placeholder the model is meant
/// to replace, not a plausible-looking invented value.
fn example_input_map(summary: &WorkflowSummary) -> Map<String, Value> {
    let mut map = Map::new();
    for input in &summary.inputs {
        let value = match (&input.default, input.ty) {
            (Some(default), _) => default.clone(),
            (None, InputType::String) => Value::String(format!("<{}>", input.name)),
            (None, InputType::Number) => json!(0),
            (None, InputType::Boolean) => json!(false),
            (None, InputType::Json) => json!({}),
        };
        map.insert(input.name.clone(), value);
    }
    map
}

/// The `argument-hint` line: required inputs in angle brackets, optional in
/// square ones, matching what both harnesses show beside a slash command.
fn argument_hint(inputs: &[WorkflowInput]) -> String {
    if inputs.is_empty() {
        return "(no inputs)".to_string();
    }
    inputs
        .iter()
        .map(|input| {
            if input.required {
                format!("<{}>", input.name)
            } else {
                format!("[{}]", input.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A double-quoted YAML scalar, safe for any one-line description.
///
/// Frontmatter is parsed before the harness ever sees the body, so a colon in a
/// workflow description must not be able to break the document.
fn yaml_scalar(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Keeps a description with a pipe in it inside its markdown table cell.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Renders a workflow id as a single shell word.
///
/// Ids are normally plain, but the fallback line is a command an operator will
/// paste, so an id with a space in it must not silently become two arguments.
fn shell_quote_arg(id: &str) -> String {
    if id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && !id.is_empty()
    {
        id.to_string()
    } else {
        format!("'{}'", id.replace('\'', "'\\''"))
    }
}
