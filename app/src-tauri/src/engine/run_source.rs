use super::{AssertionReport, ErrorKind, Verdict};
use bridge_runtime_protocol::{SnapshotId, MAX_FILE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Validated immutable source. Assertion values never form part of either prompt.
#[derive(Clone)]
pub struct ParsedPair {
    snapshot: SnapshotId,
    agent: ParsedAgent,
    cases: Vec<RunCase>,
}
/// The saved agent configuration, independent of any test file.
#[derive(Clone)]
pub struct ParsedAgent {
    prompt: String,
    name: Option<String>,
    description: Option<String>,
    model: String,
    tools: Vec<String>,
    declared_tools: Vec<DeclaredTool>,
    soul: Option<Soul>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunCase {
    pub id: String,
    input: String,
    assertions: Vec<Assertion>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Assertion {
    #[serde(rename = "contains")]
    Contains { id: String, value: String },
    #[serde(rename = "toolCalled")]
    ToolCalled { id: String, tool: String },
}
impl Assertion {
    pub fn id(&self) -> &str {
        match self {
            Self::Contains { id, .. } | Self::ToolCalled { id, .. } => id,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentHeader {
    name: Option<String>,
    description: Option<String>,
    model: String,
    kmx: Kmx,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Kmx {
    schema_version: u32,
    runtime: String,
    #[serde(default)]
    tools: Vec<DeclaredTool>,
    #[serde(default, deserialize_with = "non_null")]
    soul: Option<Soul>,
}
// Missing optional fields stay absent; explicit null is not a supported source.
fn non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("null is not a supported soul field"))
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Soul {
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    inline: Option<String>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    config_map_ref: Option<SoulReference>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    digest: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoulReference {
    name: String,
    key: String,
}
impl Soul {
    fn valid(&self) -> bool {
        if self.inline.is_some() == self.config_map_ref.is_some() {
            return false;
        }
        if let Some(digest) = &self.digest {
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest.as_bytes()[7..]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
            {
                return false;
            }
        }
        if let Some(text) = &self.inline {
            return present(text)
                && text.len() <= 8192
                && self.digest.as_ref().is_none_or(|digest| {
                    *digest == format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
                });
        }
        let reference = self.config_map_ref.as_ref().unwrap();
        let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
        // Match kmx's object-name and reference-key grammar, not a path or namespace.
        self.digest.is_some()
            && reference.name.len() <= 253
            && reference.name.split('.').all(|label| {
                !label.is_empty()
                    && alnum(label.as_bytes()[0])
                    && alnum(*label.as_bytes().last().unwrap())
                    && label.bytes().all(|c| alnum(c) || c == b'-')
            })
            && !reference.key.is_empty()
            && reference.key.len() <= 253
            && reference.key != "."
            && !reference.key.starts_with("..")
            && reference
                .key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    }
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Builtin,
    #[serde(rename = "tool")]
    Resource,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredTool {
    pub kind: ToolKind,
    pub name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestsHeader {
    schema_version: u32,
}
fn frontmatter(bytes: &[u8]) -> Result<(&str, &str), ErrorKind> {
    if bytes.len() > MAX_FILE {
        return Err(ErrorKind::Source);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ErrorKind::Source)?;
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or(ErrorKind::Source)?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Err(ErrorKind::Source);
    }
    let mut end = first.len();
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((&text[first.len()..end], &text[end + line.len()..]));
        }
        end += line.len();
    }
    Err(ErrorKind::Source)
}
fn yaml<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ErrorKind> {
    serde_yaml_ng::from_str(s).map_err(|_| ErrorKind::Source)
}
fn present(s: &str) -> bool {
    !s.trim().is_empty() && !s.contains('\0')
}
impl ParsedPair {
    pub fn parse(agent: &[u8], tests: &[u8]) -> Result<Self, ErrorKind> {
        let snapshot = SnapshotId::of(agent, tests);
        let agent = ParsedAgent::parse(agent)?;
        let (header, body) = frontmatter(tests)?;
        let tests_header: TestsHeader = yaml(header)?;
        if tests_header.schema_version != 1 {
            return Err(ErrorKind::Source);
        }
        let mut cases: Vec<RunCase> = Vec::new();
        let mut block = None;
        let mut other_fence = false;
        for line in body.split_inclusive('\n') {
            let marker = line.trim_end_matches(['\r', '\n']);
            if marker == "```yaml agent-test" && !other_fence {
                if block.is_some() {
                    return Err(ErrorKind::Source);
                }
                block = Some(String::new());
            } else if marker == "```" && block.is_some() {
                cases.push(yaml(&block.take().ok_or(ErrorKind::Source)?)?);
            } else if let Some(text) = &mut block {
                text.push_str(line);
            } else if marker.starts_with("```") {
                if marker.contains("agent-test") {
                    return Err(ErrorKind::Source);
                }
                other_fence = !other_fence;
            }
        }
        if block.is_some() || cases.is_empty() || cases.len() > 32 {
            return Err(ErrorKind::Source);
        }
        let mut ids = HashSet::new();
        for case in &cases {
            if !present(&case.id)
                || !ids.insert(&case.id)
                || !present(&case.input)
                || case.assertions.is_empty()
                || case.assertions.len() > 64
            {
                return Err(ErrorKind::Source);
            }
            let mut assertion_ids = HashSet::new();
            for assertion in &case.assertions {
                let value = match assertion {
                    Assertion::Contains { value, .. } => value,
                    Assertion::ToolCalled { tool, .. } => tool,
                };
                if !present(assertion.id())
                    || !assertion_ids.insert(assertion.id())
                    || !present(value)
                {
                    return Err(ErrorKind::Source);
                }
            }
        }
        Ok(Self {
            snapshot,
            agent,
            cases,
        })
    }
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }
    pub fn agent(&self) -> &ParsedAgent {
        &self.agent
    }
    pub fn prompt(&self) -> &str {
        self.agent.prompt()
    }
    pub fn name(&self) -> Option<&str> {
        self.agent.name()
    }
    pub fn description(&self) -> Option<&str> {
        self.agent.description()
    }
    pub fn model(&self) -> &str {
        self.agent.model()
    }
    pub fn tools(&self) -> &[String] {
        self.agent.tools()
    }
    pub fn declared_tools(&self) -> &[DeclaredTool] {
        self.agent.declared_tools()
    }
    pub fn soul(&self) -> Option<&Soul> {
        self.agent.soul()
    }
    pub fn cases(&self) -> &[RunCase] {
        &self.cases
    }
}
impl ParsedAgent {
    pub fn parse(bytes: &[u8]) -> Result<Self, ErrorKind> {
        let (header, prompt) = frontmatter(bytes)?;
        let agent: AgentHeader = yaml(header)?;
        if agent.kmx.schema_version != 1
            || agent.kmx.runtime != "orka"
            || !present(&agent.model)
            || !present(prompt)
            || agent.kmx.soul.as_ref().is_some_and(|soul| !soul.valid())
        {
            return Err(ErrorKind::Source);
        }
        let mut tools = Vec::new();
        for tool in &agent.kmx.tools {
            if !present(&tool.name) || tools.contains(&tool.name) {
                return Err(ErrorKind::Source);
            }
            tools.push(tool.name.clone());
        }
        Ok(Self {
            prompt: prompt.into(),
            name: agent.name,
            description: agent.description,
            model: agent.model,
            tools,
            declared_tools: agent.kmx.tools,
            soul: agent.kmx.soul,
        })
    }
    pub fn spec(&self, provider: &str) -> serde_json::Value {
        let mut spec = serde_json::json!({"providerRef":{"name":provider},"model":{"name":self.model()},"systemPrompt":{"inline":self.prompt()},"tools":self.tools().iter().map(|name| serde_json::json!({"name":name})).collect::<Vec<_>>()});
        if let Some(soul) = self.soul() {
            spec["soul"] = serde_json::to_value(soul).expect("validated soul serializes");
        }
        spec
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn tools(&self) -> &[String] {
        &self.tools
    }
    pub fn declared_tools(&self) -> &[DeclaredTool] {
        &self.declared_tools
    }
    pub fn soul(&self) -> Option<&Soul> {
        self.soul.as_ref()
    }
}
impl RunCase {
    pub fn input(&self) -> &str {
        &self.input
    }
    pub fn assertions(&self) -> &[Assertion] {
        &self.assertions
    }
    pub(super) fn evaluate(&self, answer: Option<&str>, tools: &[String]) -> Vec<AssertionReport> {
        self.assertions
            .iter()
            .map(|assertion| {
                let verdict = match assertion {
                    Assertion::Contains { value, .. } => match answer {
                        Some(text) if text.contains(value) => Verdict::Passed,
                        Some(_) => Verdict::Failed,
                        None => Verdict::Unknown,
                    },
                    Assertion::ToolCalled { tool, .. } if tools.contains(tool) => Verdict::Passed,
                    Assertion::ToolCalled { .. } => Verdict::Unknown,
                };
                AssertionReport {
                    id: assertion.id().into(),
                    kind: match assertion {
                        Assertion::Contains { .. } => "contains",
                        Assertion::ToolCalled { .. } => "toolCalled",
                    }
                    .into(),
                    expected: match assertion {
                        Assertion::Contains { value, .. } => value.clone(),
                        Assertion::ToolCalled { tool, .. } => tool.clone(),
                    },
                    verdict,
                }
            })
            .collect()
    }
}
