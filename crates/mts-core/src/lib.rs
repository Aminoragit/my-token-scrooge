#![forbid(unsafe_code)]

//! Deterministic, dependency-free policy primitives for `my-token-scrooge`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub const MAX_POLICY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Operation {
    Read,
    Write,
    Edit,
    Search,
    Shell,
    Execute,
    Mcp,
    Unknown,
}

impl Operation {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "edit" => Some(Self::Edit),
            "search" => Some(Self::Search),
            "shell" => Some(Self::Shell),
            "execute" => Some(Self::Execute),
            "mcp" => Some(Self::Mcp),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "READ",
            Self::Write => "WRITE",
            Self::Edit => "EDIT",
            Self::Search => "SEARCH",
            Self::Shell => "SHELL",
            Self::Execute => "EXECUTE",
            Self::Mcp => "MCP",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PolicyKind {
    FullBlock,
    PartialBlock,
}

impl PolicyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullBlock => "FULL_BLOCK",
            Self::PartialBlock => "PARTIAL_BLOCK",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplacementMode {
    Limit,
    SearchOnly,
    SymbolOnly,
    ErrorsOnly,
    MetadataOnly,
    PatchOnly,
    Redirect,
}

impl ReplacementMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "limit" => Some(Self::Limit),
            "search-only" => Some(Self::SearchOnly),
            "symbol-only" => Some(Self::SymbolOnly),
            "errors-only" => Some(Self::ErrorsOnly),
            "metadata-only" => Some(Self::MetadataOnly),
            "patch-only" => Some(Self::PatchOnly),
            "redirect" => Some(Self::Redirect),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Limit => "LIMIT",
            Self::SearchOnly => "SEARCH_ONLY",
            Self::SymbolOnly => "SYMBOL_ONLY",
            Self::ErrorsOnly => "ERRORS_ONLY",
            Self::MetadataOnly => "METADATA_ONLY",
            Self::PatchOnly => "PATCH_ONLY",
            Self::Redirect => "REDIRECT",
        }
    }

    fn allowed_options(self) -> &'static [&'static str] {
        match self {
            Self::Limit => &["max_lines", "max_bytes", "start_line", "end_line"],
            Self::SearchOnly => &["max_matches", "context_lines", "file_limit"],
            Self::SymbolOnly => &["symbols", "max_matches", "context_lines"],
            Self::ErrorsOnly => &["max_matches", "before", "after", "severity"],
            Self::MetadataOnly => &["include", "exclude"],
            Self::PatchOnly => &["max_changed_lines", "max_changed_files", "allowed_symbols"],
            Self::Redirect => &["target", "source_map"],
        }
    }

    fn numeric_options(self) -> &'static [&'static str] {
        match self {
            Self::Limit => &["max_lines", "max_bytes", "start_line", "end_line"],
            Self::SearchOnly => &["max_matches", "context_lines", "file_limit"],
            Self::SymbolOnly => &["max_matches", "context_lines"],
            Self::ErrorsOnly => &["max_matches", "before", "after"],
            Self::PatchOnly => &["max_changed_lines", "max_changed_files"],
            Self::MetadataOnly | Self::Redirect => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuleScope {
    Harness,
    Project,
    Mandatory,
    SessionLock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyRule {
    pub kind: PolicyKind,
    pub id: Option<String>,
    pub pattern: String,
    pub operations: BTreeSet<Operation>,
    pub all_operations: bool,
    pub replacement: Option<ReplacementMode>,
    pub options: BTreeMap<String, String>,
    pub reason: String,
    pub source_line: usize,
}

impl PolicyRule {
    pub fn matches(&self, operation: Operation, resource: &str) -> bool {
        self.matches_with_case(operation, resource, cfg!(windows))
    }

    pub fn matches_with_case(
        &self,
        operation: Operation,
        resource: &str,
        case_insensitive: bool,
    ) -> bool {
        operation != Operation::Unknown
            && (self.all_operations || self.operations.contains(&operation))
            && glob_matches_with_case(&self.pattern, resource, case_insensitive)
    }

    pub fn specificity(&self) -> i64 {
        specificity_score(&self.pattern, !self.all_operations)
    }

    fn stable_id(&self) -> String {
        self.id.clone().unwrap_or_else(|| {
            format!(
                "MTS-{}-{:016X}",
                match self.kind {
                    PolicyKind::FullBlock => "FULL",
                    PolicyKind::PartialBlock => "PARTIAL",
                },
                fnv1a64(self.semantic_key().as_bytes())
            )
        })
    }

    fn semantic_key(&self) -> String {
        let operations = if self.all_operations {
            "all".to_string()
        } else {
            self.operations
                .iter()
                .map(|operation| operation.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            "{}|{}|{}|{:?}|{:?}",
            self.kind.as_str(),
            normalize_resource(&self.pattern, false),
            operations,
            self.replacement,
            self.options
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyWarning {
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledPolicy {
    pub kind: PolicyKind,
    pub rules: Vec<PolicyRule>,
    pub warnings: Vec<PolicyWarning>,
}

impl CompiledPolicy {
    pub fn parse_full(source: &str) -> Result<Self, PolicyParseError> {
        parse_policy(PolicyKind::FullBlock, source)
    }

    pub fn parse_partial(source: &str) -> Result<Self, PolicyParseError> {
        parse_policy(PolicyKind::PartialBlock, source)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyParseError {
    pub line: usize,
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for PolicyParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at line {}: {}",
            self.code, self.line, self.message
        )
    }
}

impl std::error::Error for PolicyParseError {}

fn parse_policy(kind: PolicyKind, source: &str) -> Result<CompiledPolicy, PolicyParseError> {
    if source.len() > MAX_POLICY_BYTES {
        return Err(parse_error(
            0,
            "Policy input exceeds the 1 MiB safety limit.",
        ));
    }

    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    let mut semantic_lines = HashMap::<String, usize>::new();

    for (index, original) in source.lines().enumerate() {
        let line_number = index + 1;
        let without_comment = strip_comment(original, line_number)?;
        if without_comment.trim().is_empty() {
            continue;
        }
        let fields = split_fields(&without_comment, line_number)?;
        let expected = match kind {
            PolicyKind::FullBlock => 3,
            PolicyKind::PartialBlock => 5,
        };
        if fields.len() != expected {
            return Err(parse_error(
                line_number,
                &format!("Expected {expected} pipe-separated fields."),
            ));
        }

        let (id, pattern) = parse_id_and_pattern(&fields[0], line_number)?;
        let (operations, all_operations) = parse_operations(&fields[1], line_number)?;
        let reason = unquote(fields.last().expect("field count checked"), line_number)?;
        if reason.trim().is_empty() {
            return Err(parse_error(line_number, "The reason must not be empty."));
        }

        let (replacement, options) = if kind == PolicyKind::PartialBlock {
            let mode_text = unquote(&fields[2], line_number)?;
            let replacement = ReplacementMode::parse(&mode_text).ok_or_else(|| {
                parse_error(
                    line_number,
                    &format!("Unknown replacement mode '{mode_text}'."),
                )
            })?;
            let options = parse_options(replacement, &fields[3], line_number)?;
            (Some(replacement), options)
        } else {
            (None, BTreeMap::new())
        };

        let rule = PolicyRule {
            kind,
            id,
            pattern,
            operations,
            all_operations,
            replacement,
            options,
            reason: reason.trim().to_string(),
            source_line: line_number,
        };
        let semantic_key = rule.semantic_key();
        if let Some(first_line) = semantic_lines.insert(semantic_key, line_number) {
            warnings.push(PolicyWarning {
                line: line_number,
                code: "MTS_POLICY_DUPLICATE_RULE",
                message: format!("This rule duplicates the rule at line {first_line}."),
            });
        }
        rules.push(rule);
    }

    Ok(CompiledPolicy {
        kind,
        rules,
        warnings,
    })
}

fn parse_error(line: usize, message: &str) -> PolicyParseError {
    PolicyParseError {
        line,
        code: "MTS_POLICY_PARSE_ERROR",
        message: message.to_string(),
    }
}

fn strip_comment(line: &str, line_number: usize) -> Result<String, PolicyParseError> {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote.is_some() {
            escaped = true;
        } else if Some(character) == quote {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == '#' {
            return Ok(line[..index].to_string());
        }
    }
    if quote.is_some() {
        Err(parse_error(line_number, "The quoted field is not closed."))
    } else {
        Ok(line.to_string())
    }
}

fn split_fields(line: &str, line_number: usize) -> Result<Vec<String>, PolicyParseError> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote.is_some() {
            escaped = true;
        } else if Some(character) == quote {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == '|' {
            fields.push(line[start..index].trim().to_string());
            start = index + character.len_utf8();
        }
    }
    if quote.is_some() {
        return Err(parse_error(line_number, "The quoted field is not closed."));
    }
    fields.push(line[start..].trim().to_string());
    Ok(fields)
}

fn unquote(value: &str, line_number: usize) -> Result<String, PolicyParseError> {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if matches!(first, '\'' | '"') {
            if first != last {
                return Err(parse_error(line_number, "The quoted field is not closed."));
            }
            return Ok(value[1..value.len() - 1]
                .replace(&format!("\\{first}"), &first.to_string())
                .replace("\\\\", "\\"));
        }
    }
    Ok(value.to_string())
}

fn parse_id_and_pattern(
    field: &str,
    line_number: usize,
) -> Result<(Option<String>, String), PolicyParseError> {
    let field = unquote(field, line_number)?;
    let trimmed = field.trim();
    let (id, pattern) = if let Some(rest) = trimmed.strip_prefix('@') {
        let boundary = rest.find(char::is_whitespace).ok_or_else(|| {
            parse_error(
                line_number,
                "A rule identifier must be followed by a path pattern.",
            )
        })?;
        let id = &rest[..boundary];
        if id.is_empty()
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(parse_error(
                line_number,
                "Rule identifiers may contain only ASCII letters, digits, '-' and '_'.",
            ));
        }
        (Some(id.to_ascii_uppercase()), rest[boundary..].trim())
    } else {
        (None, trimmed)
    };
    if pattern.is_empty() {
        return Err(parse_error(
            line_number,
            "The path pattern must not be empty.",
        ));
    }
    Ok((id, normalize_resource(pattern, false)))
}

fn parse_operations(
    field: &str,
    line_number: usize,
) -> Result<(BTreeSet<Operation>, bool), PolicyParseError> {
    let mut operations = BTreeSet::new();
    let mut all_operations = false;
    for item in unquote(field, line_number)?.split(',') {
        let value = item.trim();
        if value.is_empty() {
            return Err(parse_error(
                line_number,
                "Operation names must not be empty.",
            ));
        }
        if value.eq_ignore_ascii_case("all") {
            all_operations = true;
        } else {
            let operation = Operation::parse(value).ok_or_else(|| {
                parse_error(line_number, &format!("Unknown operation '{value}'."))
            })?;
            if !operations.insert(operation) {
                return Err(parse_error(
                    line_number,
                    &format!("Operation '{value}' is listed more than once."),
                ));
            }
        }
    }
    if all_operations && !operations.is_empty() {
        return Err(parse_error(
            line_number,
            "Operation 'all' cannot be combined with explicit operations.",
        ));
    }
    if !all_operations && operations.is_empty() {
        return Err(parse_error(
            line_number,
            "At least one operation is required.",
        ));
    }
    Ok((operations, all_operations))
}

fn parse_options(
    mode: ReplacementMode,
    field: &str,
    line_number: usize,
) -> Result<BTreeMap<String, String>, PolicyParseError> {
    let field = unquote(field, line_number)?;
    let mut options = BTreeMap::new();
    if field.trim().is_empty() {
        return Ok(options);
    }
    for item in field.split(',') {
        let (key, value) = item.split_once('=').ok_or_else(|| {
            parse_error(
                line_number,
                "Each replacement option must use key=value syntax.",
            )
        })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if !mode.allowed_options().contains(&key.as_str()) {
            return Err(parse_error(
                line_number,
                &format!("Option '{key}' is not valid for {}.", mode.as_str()),
            ));
        }
        if value.is_empty() {
            return Err(parse_error(
                line_number,
                &format!("Option '{key}' must have a value."),
            ));
        }
        if mode.numeric_options().contains(&key.as_str())
            && value
                .parse::<u64>()
                .ok()
                .filter(|number| *number > 0)
                .is_none()
        {
            return Err(parse_error(
                line_number,
                &format!("Option '{key}' must be a positive integer."),
            ));
        }
        if options.insert(key.clone(), value.to_string()).is_some() {
            return Err(parse_error(
                line_number,
                &format!("Option '{key}' is listed more than once."),
            ));
        }
    }

    if mode == ReplacementMode::Limit {
        let start = options
            .get("start_line")
            .and_then(|value| value.parse::<u64>().ok());
        let end = options
            .get("end_line")
            .and_then(|value| value.parse::<u64>().ok());
        if matches!((start, end), (Some(start), Some(end)) if start > end) {
            return Err(parse_error(
                line_number,
                "Option 'start_line' must not exceed 'end_line'.",
            ));
        }
    }
    Ok(options)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScopedRule {
    rule: PolicyRule,
    scope: RuleScope,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PolicySet {
    rules: Vec<ScopedRule>,
}

impl PolicySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, policy: CompiledPolicy, scope: RuleScope) {
        self.rules.extend(
            policy
                .rules
                .into_iter()
                .map(|rule| ScopedRule { rule, scope }),
        );
    }

    pub fn decide(&self, operation: Operation, resource: &str) -> Decision {
        self.decide_with_case(operation, resource, cfg!(windows))
    }

    pub fn decide_with_case(
        &self,
        operation: Operation,
        resource: &str,
        case_insensitive: bool,
    ) -> Decision {
        let normalized = normalize_resource(resource, case_insensitive);
        let selected = self
            .rules
            .iter()
            .filter(|entry| {
                entry
                    .rule
                    .matches_with_case(operation, &normalized, case_insensitive)
            })
            .max_by(compare_scoped_rules);
        let Some(entry) = selected else {
            return Decision::Allow;
        };
        let rule_id = entry.rule.stable_id();
        match entry.rule.kind {
            PolicyKind::FullBlock => Decision::FullBlock(FullBlockDecision {
                rule_id,
                reason_code: "MTS_POLICY_FULL_BLOCK",
                reason: entry.rule.reason.clone(),
                matched_pattern: entry.rule.pattern.clone(),
                retry_allowed: false,
                approval_possible: false,
            }),
            PolicyKind::PartialBlock => Decision::PartialBlock(PartialBlockDecision {
                rule_id,
                reason_code: "MTS_POLICY_PARTIAL_BLOCK",
                reason: entry.rule.reason.clone(),
                matched_pattern: entry.rule.pattern.clone(),
                substitute: SubstituteMetadata {
                    mode: entry
                        .rule
                        .replacement
                        .expect("partial rules always have a mode"),
                    bounds: entry.rule.options.clone(),
                    original_executed: false,
                    truncated: None,
                    omitted_bytes: None,
                    returned_bytes: None,
                },
            }),
        }
    }
}

fn compare_scoped_rules(left: &&ScopedRule, right: &&ScopedRule) -> Ordering {
    rule_rank(left)
        .cmp(&rule_rank(right))
        .then_with(|| right.rule.semantic_key().cmp(&left.rule.semantic_key()))
}

fn rule_rank(rule: &ScopedRule) -> (u8, i64, u8, u8, u8) {
    let absolute_priority = match rule.scope {
        RuleScope::Mandatory => 2,
        RuleScope::SessionLock => 1,
        RuleScope::Harness | RuleScope::Project => 0,
    };
    (
        absolute_priority,
        rule.rule.specificity(),
        u8::from(rule.scope == RuleScope::Project),
        u8::from(rule.rule.kind == PolicyKind::FullBlock),
        u8::from(!rule.rule.all_operations),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    FullBlock(FullBlockDecision),
    PartialBlock(PartialBlockDecision),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullBlockDecision {
    pub rule_id: String,
    pub reason_code: &'static str,
    pub reason: String,
    pub matched_pattern: String,
    pub retry_allowed: bool,
    pub approval_possible: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialBlockDecision {
    pub rule_id: String,
    pub reason_code: &'static str,
    pub reason: String,
    pub matched_pattern: String,
    pub substitute: SubstituteMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstituteMetadata {
    pub mode: ReplacementMode,
    pub bounds: BTreeMap<String, String>,
    pub original_executed: bool,
    pub truncated: Option<bool>,
    pub omitted_bytes: Option<u64>,
    pub returned_bytes: Option<u64>,
}

impl SubstituteMetadata {
    pub fn complete(mut self, returned_bytes: u64, omitted_bytes: u64) -> Self {
        self.returned_bytes = Some(returned_bytes);
        self.omitted_bytes = Some(omitted_bytes);
        self.truncated = Some(omitted_bytes > 0);
        self
    }
}

pub fn specificity_score(pattern: &str, explicit_operation: bool) -> i64 {
    let pattern = normalize_resource(pattern, false);
    let literal_characters = pattern
        .chars()
        .filter(|character| !matches!(character, '*' | '?'))
        .count() as i64;
    let exact_segments = pattern
        .split('/')
        .filter(|segment| !segment.contains(['*', '?']) && !segment.is_empty())
        .count() as i64;
    let recursive_wildcards = pattern.matches("**").count() as i64;
    let wildcard_characters = pattern.matches('*').count() as i64
        + pattern.matches('?').count() as i64
        - recursive_wildcards * 2;
    exact_segments * 1_000 + literal_characters * 10
        - recursive_wildcards * 100
        - wildcard_characters * 10
        + i64::from(explicit_operation) * 5
}

pub fn glob_matches(pattern: &str, resource: &str) -> bool {
    glob_matches_with_case(pattern, resource, cfg!(windows))
}

pub fn glob_matches_with_case(pattern: &str, resource: &str, case_insensitive: bool) -> bool {
    let pattern = normalize_resource(pattern, false);
    let pattern = if case_insensitive {
        pattern.to_ascii_lowercase()
    } else {
        pattern
    };
    let resource = normalize_resource(resource, case_insensitive);
    if glob_match_anchored(pattern.as_bytes(), resource.as_bytes()) {
        return true;
    }
    if pattern.starts_with('/') || has_drive_prefix(&pattern) {
        return false;
    }
    resource.match_indices('/').any(|(index, _)| {
        glob_match_anchored(pattern.as_bytes(), &resource.as_bytes()[index + 1..])
    })
}

fn glob_match_anchored(pattern: &[u8], value: &[u8]) -> bool {
    fn matches(
        pattern: &[u8],
        value: &[u8],
        p: usize,
        v: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(result) = memo.get(&(p, v)) {
            return *result;
        }
        let result = if p == pattern.len() {
            v == value.len()
        } else if pattern[p] == b'*' && pattern.get(p + 1) == Some(&b'*') {
            let mut after = p + 2;
            while pattern.get(after) == Some(&b'*') {
                after += 1;
            }
            if pattern.get(after) == Some(&b'/') {
                matches(pattern, value, after + 1, v, memo)
                    || (v < value.len() && matches(pattern, value, p, v + 1, memo))
            } else {
                matches(pattern, value, after, v, memo)
                    || (v < value.len() && matches(pattern, value, p, v + 1, memo))
            }
        } else if pattern[p] == b'*' {
            matches(pattern, value, p + 1, v, memo)
                || (v < value.len() && value[v] != b'/' && matches(pattern, value, p, v + 1, memo))
        } else if pattern[p] == b'?' {
            v < value.len() && value[v] != b'/' && matches(pattern, value, p + 1, v + 1, memo)
        } else {
            v < value.len() && pattern[p] == value[v] && matches(pattern, value, p + 1, v + 1, memo)
        };
        memo.insert((p, v), result);
        result
    }
    matches(pattern, value, 0, 0, &mut HashMap::new())
}

fn has_drive_prefix(path: &str) -> bool {
    path.as_bytes().get(1) == Some(&b':')
}

pub fn normalize_resource(resource: &str, case_insensitive: bool) -> String {
    let replaced = resource.trim().replace('\\', "/");
    let mut prefix = "";
    let mut body = replaced.as_str();
    if let Some(rest) = body.strip_prefix("//") {
        prefix = "//";
        body = rest;
    } else if let Some(rest) = body.strip_prefix('/') {
        prefix = "/";
        body = rest;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in body.split('/') {
        match segment {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            ".." if prefix.is_empty() => segments.push(segment),
            ".." => {}
            _ => segments.push(segment),
        }
    }
    let mut normalized = format!("{prefix}{}", segments.join("/"));
    if normalized.is_empty() {
        normalized.push('.');
    }
    if case_insensitive {
        normalized.make_ascii_lowercase();
    } else if has_drive_prefix(&normalized) {
        normalized.replace_range(0..1, &normalized[..1].to_ascii_lowercase());
    }
    normalized
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IntentClass {
    ExactRead,
    RecursiveRead,
    BoundedRead,
    TextSearch,
    SymbolSearch,
    DirectEdit,
    PatchEdit,
    DirectWrite,
    DirectExecution,
    ArchiveExpansion,
    MetadataRequest,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResourceIntentKey {
    pub target_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub intent_class: IntentClass,
    pub canonical_resource: String,
    pub operation: Operation,
    pub rule_id: String,
    pub remote_executor_id: Option<String>,
}

impl Hash for ResourceIntentKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.target_id.hash(state);
        self.session_id.hash(state);
        self.workspace_id.hash(state);
        self.intent_class.hash(state);
        self.canonical_resource.hash(state);
        self.operation.hash(state);
        self.rule_id.hash(state);
        self.remote_executor_id.hash(state);
    }
}

impl ResourceIntentKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_id: impl Into<String>,
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
        intent_class: IntentClass,
        canonical_resource: impl AsRef<str>,
        operation: Operation,
        rule_id: impl Into<String>,
        remote_executor_id: Option<String>,
        case_insensitive_resource: bool,
    ) -> Self {
        Self {
            target_id: target_id.into().trim().to_ascii_lowercase(),
            session_id: session_id.into().trim().to_string(),
            workspace_id: workspace_id.into().trim().to_string(),
            intent_class,
            canonical_resource: normalize_resource(
                canonical_resource.as_ref(),
                case_insensitive_resource,
            ),
            operation,
            rule_id: rule_id.into().trim().to_ascii_uppercase(),
            remote_executor_id: remote_executor_id.map(|id| id.trim().to_ascii_lowercase()),
        }
    }
}

pub const MAX_SHELL_COMMAND_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellFamily {
    Unix,
    PowerShell,
    Cmd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellResourceIntent {
    pub operation: Operation,
    pub intent_class: IntentClass,
    pub resource: String,
    pub search_query: Option<String>,
    pub line_range: Option<(u64, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellParseError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ShellParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ShellParseError {}

/// Extracts literal resource intents without executing or expanding the command.
pub fn extract_shell_intents(
    command: &str,
    family: ShellFamily,
) -> Result<Vec<ShellResourceIntent>, ShellParseError> {
    if command.len() > MAX_SHELL_COMMAND_BYTES {
        return Err(shell_error("Shell input exceeds the 64 KiB safety limit."));
    }
    if has_dynamic_shell_expansion(command, family)? {
        return Err(ShellParseError {
            code: "MTS_SHELL_AMBIGUOUS",
            message: "Dynamic shell expansion cannot be normalized without execution.".to_string(),
        });
    }
    let commands = tokenize_shell(command, family)?;
    let mut intents = Vec::new();
    for arguments in commands {
        if arguments.is_empty() {
            continue;
        }
        extract_command_intents(&arguments, family, &mut intents)?;
    }
    Ok(intents)
}

fn shell_error(message: &str) -> ShellParseError {
    ShellParseError {
        code: "MTS_SHELL_PARSE_ERROR",
        message: message.to_string(),
    }
}

fn has_dynamic_shell_expansion(
    command: &str,
    family: ShellFamily,
) -> Result<bool, ShellParseError> {
    let mut quote = None;
    let mut escaped = false;
    let characters: Vec<char> = command.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if escaped {
            escaped = false;
        } else if character == '\\' && family != ShellFamily::PowerShell {
            escaped = true;
        } else if Some(character) == quote {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote != Some('\'')
            && ((character == '$' && characters.get(index + 1) == Some(&'('))
                || (character == '`' && family == ShellFamily::Unix)
                || (character == '%' && family == ShellFamily::Cmd))
        {
            return Ok(true);
        }
        index += 1;
    }
    if quote.is_some() {
        Err(shell_error("The shell quote is not closed."))
    } else {
        Ok(false)
    }
}

fn tokenize_shell(command: &str, family: ShellFamily) -> Result<Vec<Vec<String>>, ShellParseError> {
    let mut commands = vec![Vec::new()];
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        if escaped {
            token.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && family != ShellFamily::PowerShell && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if Some(character) == quote {
            quote = None;
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if quote.is_none() && (character.is_whitespace() || matches!(character, ';' | '|' | '&')) {
            if !token.is_empty() {
                commands
                    .last_mut()
                    .expect("one command exists")
                    .push(std::mem::take(&mut token));
            }
            if matches!(character, ';' | '|' | '&') {
                while characters.peek().is_some_and(|next| *next == character) {
                    characters.next();
                }
                if !commands.last().expect("one command exists").is_empty() {
                    commands.push(Vec::new());
                }
            }
            continue;
        }
        token.push(character);
    }
    if escaped || quote.is_some() {
        return Err(shell_error("The shell quote or escape is not closed."));
    }
    if !token.is_empty() {
        commands.last_mut().expect("one command exists").push(token);
    }
    commands.retain(|arguments| !arguments.is_empty());
    Ok(commands)
}

fn extract_command_intents(
    arguments: &[String],
    family: ShellFamily,
    output: &mut Vec<ShellResourceIntent>,
) -> Result<(), ShellParseError> {
    let executable = arguments[0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&arguments[0])
        .trim_end_matches(".exe")
        .to_ascii_lowercase();

    if matches!(executable.as_str(), "bash" | "sh") {
        if let Some(script) = argument_after(arguments, "-c") {
            output.extend(extract_shell_intents(script, ShellFamily::Unix)?);
        }
        return Ok(());
    }
    if matches!(executable.as_str(), "powershell" | "pwsh") {
        if let Some(script) = argument_after_case_insensitive(arguments, &["-command", "-c"]) {
            output.extend(extract_shell_intents(script, ShellFamily::PowerShell)?);
        }
        return Ok(());
    }
    if executable == "cmd" {
        if let Some(script) = argument_after_case_insensitive(arguments, &["/c"]) {
            output.extend(extract_shell_intents(script, ShellFamily::Cmd)?);
        }
        return Ok(());
    }
    if matches!(executable.as_str(), "python" | "python3" | "node") {
        if let Some(script) = argument_after_case_insensitive(arguments, &["-c", "-e"]) {
            for resource in extract_literal_calls(script) {
                output.push(shell_intent(
                    Operation::Read,
                    IntentClass::ExactRead,
                    &resource,
                ));
            }
        }
        return Ok(());
    }

    match family {
        ShellFamily::Unix => extract_unix_intents(&executable, arguments, output),
        ShellFamily::PowerShell => extract_powershell_intents(&executable, arguments, output),
        ShellFamily::Cmd => extract_cmd_intents(&executable, arguments, output),
    }
}

fn extract_unix_intents(
    executable: &str,
    arguments: &[String],
    output: &mut Vec<ShellResourceIntent>,
) -> Result<(), ShellParseError> {
    let operands = shell_operands(arguments);
    match executable {
        "cat" | "less" | "wc" => {
            push_resources(output, &operands, Operation::Read, IntentClass::ExactRead)
        }
        "head" | "tail" | "sed" | "awk" => {
            if let Some(resource) = operands.last() {
                let mut intent = shell_intent(Operation::Read, IntentClass::BoundedRead, resource);
                intent.line_range = shell_line_range(arguments, executable);
                output.push(intent);
            }
        }
        "grep" | "rg" => push_search_intents(output, &operands),
        "find" | "ls" | "tree" => {
            let resource = operands.first().map(String::as_str).unwrap_or(".");
            output.push(shell_intent(
                Operation::Read,
                IntentClass::RecursiveRead,
                resource,
            ));
        }
        "cp" | "mv" if operands.len() >= 2 => {
            output.push(shell_intent(
                Operation::Read,
                IntentClass::ExactRead,
                &operands[0],
            ));
            output.push(shell_intent(
                Operation::Write,
                IntentClass::DirectWrite,
                operands.last().unwrap(),
            ));
        }
        "rm" => push_resources(
            output,
            &operands,
            Operation::Write,
            IntentClass::DirectWrite,
        ),
        "tar" | "zip" | "unzip" => push_resources(
            output,
            &operands,
            Operation::Read,
            IntentClass::ArchiveExpansion,
        ),
        _ => {}
    }
    Ok(())
}

fn extract_powershell_intents(
    executable: &str,
    arguments: &[String],
    output: &mut Vec<ShellResourceIntent>,
) -> Result<(), ShellParseError> {
    let operands = shell_operands(arguments);
    match executable {
        "get-content" => push_resources(output, &operands, Operation::Read, IntentClass::ExactRead),
        "select-string" => push_search_intents(output, &operands),
        "get-childitem" | "gci" | "dir" => {
            let resource = operands.first().map(String::as_str).unwrap_or(".");
            let recursive = arguments
                .iter()
                .any(|value| value.eq_ignore_ascii_case("-recurse"));
            output.push(shell_intent(
                Operation::Read,
                if recursive {
                    IntentClass::RecursiveRead
                } else {
                    IntentClass::MetadataRequest
                },
                resource,
            ));
        }
        "set-content" | "add-content" | "remove-item" => push_resources(
            output,
            &operands,
            Operation::Write,
            IntentClass::DirectWrite,
        ),
        "copy-item" | "move-item" if operands.len() >= 2 => {
            output.push(shell_intent(
                Operation::Read,
                IntentClass::ExactRead,
                &operands[0],
            ));
            output.push(shell_intent(
                Operation::Write,
                IntentClass::DirectWrite,
                operands.last().unwrap(),
            ));
        }
        _ => {}
    }
    Ok(())
}

fn extract_cmd_intents(
    executable: &str,
    arguments: &[String],
    output: &mut Vec<ShellResourceIntent>,
) -> Result<(), ShellParseError> {
    let operands = shell_operands(arguments);
    match executable {
        "type" => push_resources(output, &operands, Operation::Read, IntentClass::ExactRead),
        "dir" => {
            let resource = operands.first().map(String::as_str).unwrap_or(".");
            output.push(shell_intent(
                Operation::Read,
                IntentClass::RecursiveRead,
                resource,
            ));
        }
        "findstr" => push_search_intents(output, &operands),
        "copy" | "move" if operands.len() >= 2 => {
            output.push(shell_intent(
                Operation::Read,
                IntentClass::ExactRead,
                &operands[0],
            ));
            output.push(shell_intent(
                Operation::Write,
                IntentClass::DirectWrite,
                operands.last().unwrap(),
            ));
        }
        "del" | "erase" => push_resources(
            output,
            &operands,
            Operation::Write,
            IntentClass::DirectWrite,
        ),
        _ => {}
    }
    Ok(())
}

fn shell_operands(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .skip(1)
        .filter(|value| {
            !value.starts_with('-')
                && !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "/a" | "/b" | "/c" | "/f" | "/q" | "/s"
                )
        })
        .cloned()
        .collect()
}

fn shell_line_range(arguments: &[String], executable: &str) -> Option<(u64, u64)> {
    if executable == "head" {
        for (index, argument) in arguments.iter().enumerate().skip(1) {
            let value = argument
                .strip_prefix("-n")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    (argument == "-n")
                        .then(|| arguments.get(index + 1))
                        .flatten()
                        .map(String::as_str)
                });
            if let Some(lines) = value.and_then(|value| value.parse::<u64>().ok()) {
                return Some((1, lines));
            }
        }
    }
    if executable == "sed" {
        for argument in arguments.iter().skip(1) {
            let range = argument.trim_end_matches('p');
            if let Some((start, end)) = range.split_once(',') {
                if let (Ok(start), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) {
                    return Some((start, end));
                }
            }
        }
    }
    None
}

fn push_resources(
    output: &mut Vec<ShellResourceIntent>,
    resources: &[String],
    operation: Operation,
    intent_class: IntentClass,
) {
    output.extend(
        resources
            .iter()
            .map(|resource| shell_intent(operation, intent_class, resource)),
    );
}

fn push_search_intents(output: &mut Vec<ShellResourceIntent>, operands: &[String]) {
    let Some(query) = operands.first() else {
        return;
    };
    let resources = if operands.len() > 1 {
        &operands[1..]
    } else {
        &[]
    };
    if resources.is_empty() {
        let mut intent = shell_intent(Operation::Search, IntentClass::TextSearch, ".");
        intent.search_query = Some(query.clone());
        output.push(intent);
    } else {
        for resource in resources {
            let mut intent = shell_intent(Operation::Search, IntentClass::TextSearch, resource);
            intent.search_query = Some(query.clone());
            output.push(intent);
        }
    }
}

fn shell_intent(
    operation: Operation,
    intent_class: IntentClass,
    resource: &str,
) -> ShellResourceIntent {
    ShellResourceIntent {
        operation,
        intent_class,
        resource: normalize_resource(resource, false),
        search_query: None,
        line_range: None,
    }
}

fn argument_after<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

fn argument_after_case_insensitive<'a>(arguments: &'a [String], flags: &[&str]) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| flags.iter().any(|flag| argument.eq_ignore_ascii_case(flag)))
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

fn extract_literal_calls(script: &str) -> Vec<String> {
    const MARKERS: &[&str] = &["open(", "readfilesync(", "read_to_string("];
    let lowercase = script.to_ascii_lowercase();
    let mut resources = Vec::new();
    for marker in MARKERS {
        let mut offset = 0;
        while let Some(relative) = lowercase[offset..].find(marker) {
            let start = offset + relative + marker.len();
            let remainder = script[start..].trim_start();
            let Some(quote) = remainder
                .chars()
                .next()
                .filter(|value| matches!(value, '\'' | '"'))
            else {
                offset = start;
                continue;
            };
            if let Some(end) = remainder[1..].find(quote) {
                resources.push(normalize_resource(&remainder[1..end + 1], false));
            }
            offset = start;
        }
    }
    resources
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryConfig {
    pub same_resource_limit: u32,
    pub same_rule_limit: u32,
    pub window_seconds: u64,
    pub final_block_ttl_seconds: u64,
    pub max_alias_variants: usize,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            same_resource_limit: 2,
            same_rule_limit: 4,
            window_seconds: 120,
            final_block_ttl_seconds: 1_800,
            max_alias_variants: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryProgress {
    None,
    NarrowerResource,
    SmallerBounds,
    PatchInsteadOfEdit,
    UserApproved,
    DifferentRelevantResource,
}

impl RetryProgress {
    const fn is_meaningful(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryState {
    BlockedWithGuidance,
    SubstituteReturned,
    CircuitOpen,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryOutcome {
    pub state: RetryState,
    pub equivalent_attempts: u32,
    pub compact_code: Option<String>,
}

#[derive(Clone, Debug)]
struct RetryRecord {
    attempts: u32,
    first_at: u64,
    last_at: u64,
    opened_at: Option<u64>,
    aliases: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RuleRetryKey {
    target_id: String,
    session_id: String,
    workspace_id: String,
    rule_id: String,
}

#[derive(Clone, Debug)]
pub struct RetryCircuit {
    config: RetryConfig,
    resources: HashMap<ResourceIntentKey, RetryRecord>,
    rules: HashMap<RuleRetryKey, Vec<u64>>,
}

impl RetryCircuit {
    pub fn new(config: RetryConfig) -> Self {
        Self {
            config,
            resources: HashMap::new(),
            rules: HashMap::new(),
        }
    }

    pub fn record_attempt(
        &mut self,
        key: ResourceIntentKey,
        raw_alias: Option<&str>,
        now_seconds: u64,
        progress: RetryProgress,
    ) -> RetryOutcome {
        let rule_key = RuleRetryKey {
            target_id: key.target_id.clone(),
            session_id: key.session_id.clone(),
            workspace_id: key.workspace_id.clone(),
            rule_id: key.rule_id.clone(),
        };
        if progress.is_meaningful() {
            self.resources.remove(&key);
            self.rules.remove(&rule_key);
        }

        let record = self
            .resources
            .entry(key.clone())
            .or_insert_with(|| RetryRecord {
                attempts: 0,
                first_at: now_seconds,
                last_at: now_seconds,
                opened_at: None,
                aliases: BTreeSet::new(),
            });
        let expired_window =
            now_seconds.saturating_sub(record.last_at) > self.config.window_seconds;
        let expired_circuit = record.opened_at.is_some_and(|opened_at| {
            now_seconds.saturating_sub(opened_at) > self.config.final_block_ttl_seconds
        });
        if if record.opened_at.is_some() {
            expired_circuit
        } else {
            expired_window
        } {
            *record = RetryRecord {
                attempts: 0,
                first_at: now_seconds,
                last_at: now_seconds,
                opened_at: None,
                aliases: BTreeSet::new(),
            };
            self.rules.remove(&rule_key);
        }

        record.attempts = record.attempts.saturating_add(1);
        record.last_at = now_seconds;
        if let Some(alias) = raw_alias {
            record
                .aliases
                .insert(normalize_resource(alias, cfg!(windows)));
        }
        let rule_attempts = self.rules.entry(rule_key).or_default();
        rule_attempts.retain(|timestamp| {
            now_seconds.saturating_sub(*timestamp) <= self.config.window_seconds
        });
        rule_attempts.push(now_seconds);

        let should_open = record.opened_at.is_some()
            || record.attempts > self.config.same_resource_limit
            || rule_attempts.len() as u32 > self.config.same_rule_limit
            || record.aliases.len() > self.config.max_alias_variants;
        let state = if should_open {
            record.opened_at.get_or_insert(now_seconds);
            RetryState::CircuitOpen
        } else if record.attempts == 1 {
            RetryState::BlockedWithGuidance
        } else {
            RetryState::SubstituteReturned
        };
        RetryOutcome {
            state,
            equivalent_attempts: record.attempts,
            compact_code: (state == RetryState::CircuitOpen)
                .then(|| format!("MTS_CIRCUIT_OPEN:{}", key.rule_id)),
        }
    }

    pub fn clear_session(&mut self, session_id: &str) {
        self.resources.retain(|key, _| key.session_id != session_id);
        self.rules.retain(|key, _| key.session_id != session_id);
    }

    pub fn first_seen_at(&self, key: &ResourceIntentKey) -> Option<u64> {
        self.resources.get(key).map(|record| record.first_at)
    }
}

impl Default for RetryCircuit {
    fn default() -> Self {
        Self::new(RetryConfig::default())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EstimateConfidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavingsInput {
    pub protected_bytes: u64,
    pub avoided_output_bytes: u64,
    pub replacement_output_bytes: u64,
    pub retry_overhead_bytes: u64,
    pub bytes_per_token: u32,
    pub estimate_method: String,
    pub confidence: EstimateConfidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavingsMeasurement {
    pub protected_bytes: u64,
    pub avoided_output_bytes: u64,
    pub replacement_output_bytes: u64,
    pub retry_overhead_bytes: u64,
    pub net_avoided_bytes: u64,
    pub estimated_net_tokens_saved: u64,
    pub estimate_method: String,
    pub confidence: EstimateConfidence,
    pub first_prevention_saving: u64,
    pub replacement_cost: u64,
    pub retry_overhead: u64,
    pub loop_prevention_saving: u64,
    pub deduplicated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SavingsLedger {
    seen: HashSet<ResourceIntentKey>,
}

impl SavingsLedger {
    pub fn account(
        &mut self,
        key: ResourceIntentKey,
        input: SavingsInput,
        loop_prevention_saving: u64,
    ) -> SavingsMeasurement {
        let first = self.seen.insert(key);
        let net = input
            .avoided_output_bytes
            .saturating_sub(input.replacement_output_bytes)
            .saturating_sub(input.retry_overhead_bytes);
        let claimable_net = if first {
            net
        } else {
            loop_prevention_saving.min(net)
        };
        let bytes_per_token = u64::from(input.bytes_per_token.max(1));
        SavingsMeasurement {
            protected_bytes: input.protected_bytes,
            avoided_output_bytes: input.avoided_output_bytes,
            replacement_output_bytes: input.replacement_output_bytes,
            retry_overhead_bytes: input.retry_overhead_bytes,
            net_avoided_bytes: net,
            estimated_net_tokens_saved: claimable_net / bytes_per_token,
            estimate_method: input.estimate_method,
            confidence: input.confidence,
            first_prevention_saving: if first { net } else { 0 },
            replacement_cost: input.replacement_output_bytes,
            retry_overhead: input.retry_overhead_bytes,
            loop_prevention_saving: if first {
                0
            } else {
                loop_prevention_saving.min(net)
            },
            deduplicated: !first,
        }
    }
}

pub const MAX_REPLACEMENT_SCAN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadBounds {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub start_line: usize,
    pub end_line: Option<usize>,
}

impl Default for ReadBounds {
    fn default() -> Self {
        Self {
            max_lines: 200,
            max_bytes: 64 * 1024,
            start_line: 1,
            end_line: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedText {
    pub text: String,
    pub original_bytes: u64,
    pub returned_bytes: u64,
    pub omitted_bytes: u64,
    pub truncated: bool,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug)]
pub struct PartialExecutionError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for PartialExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PartialExecutionError {}

pub fn bounded_read(path: &Path, bounds: ReadBounds) -> Result<BoundedText, PartialExecutionError> {
    if bounds.max_lines == 0 || bounds.max_bytes == 0 || bounds.start_line == 0 {
        return Err(partial_error(
            "MTS_PARTIAL_INVALID_BOUNDS",
            "Read bounds must be positive.",
        ));
    }
    if bounds.end_line.is_some_and(|end| end < bounds.start_line) {
        return Err(partial_error(
            "MTS_PARTIAL_INVALID_BOUNDS",
            "The end line must not precede the start line.",
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| partial_io_error(path, error))?;
    if !metadata.is_file() {
        return Err(partial_error(
            "MTS_PARTIAL_UNSUPPORTED_RESOURCE",
            "Bounded reads require a regular file.",
        ));
    }
    let mut reader = BufReader::new(
        fs::File::open(path)
            .map_err(|error| partial_io_error(path, error))?
            .take(MAX_REPLACEMENT_SCAN_BYTES),
    );
    let mut output = Vec::new();
    let mut buffer = Vec::new();
    let mut line_number = 0;
    let mut returned_lines = 0;
    let mut last_returned_line = bounds.start_line.saturating_sub(1);
    let mut scanned_bytes = 0u64;

    while scanned_bytes < MAX_REPLACEMENT_SCAN_BYTES {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|error| partial_io_error(path, error))?;
        if read == 0 {
            break;
        }
        scanned_bytes = scanned_bytes.saturating_add(read as u64);
        line_number += 1;
        if buffer.contains(&0) {
            return Err(partial_error(
                "MTS_PARTIAL_BINARY_RESOURCE",
                "Binary files cannot be returned as bounded text.",
            ));
        }
        if line_number < bounds.start_line {
            continue;
        }
        if bounds.end_line.is_some_and(|end| line_number > end)
            || returned_lines >= bounds.max_lines
            || output.len() >= bounds.max_bytes
        {
            break;
        }
        let remaining = bounds.max_bytes - output.len();
        let take = remaining.min(buffer.len());
        output.extend_from_slice(&buffer[..take]);
        returned_lines += 1;
        last_returned_line = line_number;
        if take < buffer.len() {
            break;
        }
    }

    let text = utf8_bounded(output)?;
    let returned_bytes = text.len() as u64;
    let omitted_bytes = metadata.len().saturating_sub(returned_bytes);
    Ok(BoundedText {
        text,
        original_bytes: metadata.len(),
        returned_bytes,
        omitted_bytes,
        truncated: omitted_bytes > 0,
        start_line: bounds.start_line,
        end_line: last_returned_line,
    })
}

fn utf8_bounded(mut bytes: Vec<u8>) -> Result<String, PartialExecutionError> {
    loop {
        match String::from_utf8(bytes) {
            Ok(text) => return Ok(text),
            Err(error) if error.utf8_error().error_len().is_none() => {
                let valid_up_to = error.utf8_error().valid_up_to();
                bytes = error.into_bytes();
                bytes.truncate(valid_up_to);
            }
            Err(_) => {
                return Err(partial_error(
                    "MTS_PARTIAL_BINARY_RESOURCE",
                    "The selected file is not valid UTF-8 text.",
                ))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchBounds {
    pub max_matches: usize,
    pub context_lines: usize,
    pub file_limit: usize,
    pub max_scan_bytes: u64,
}

impl Default for SearchBounds {
    fn default() -> Self {
        Self {
            max_matches: 40,
            context_lines: 3,
            file_limit: 200,
            max_scan_bytes: MAX_REPLACEMENT_SCAN_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextLine {
    pub line_number: usize,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub context: Vec<ContextLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedSearchResult {
    pub matches: Vec<SearchMatch>,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub truncated: bool,
}

pub fn bounded_search(
    root: &Path,
    query: &str,
    bounds: SearchBounds,
) -> Result<BoundedSearchResult, PartialExecutionError> {
    if query.is_empty()
        || bounds.max_matches == 0
        || bounds.file_limit == 0
        || bounds.max_scan_bytes == 0
    {
        return Err(partial_error(
            "MTS_PARTIAL_INVALID_BOUNDS",
            "Search query and bounds must be non-empty and positive.",
        ));
    }
    let files = collect_regular_files(root, bounds.file_limit)?;
    let discovered_more_files = files.len() > bounds.file_limit;
    let mut result = BoundedSearchResult {
        matches: Vec::new(),
        files_scanned: 0,
        bytes_scanned: 0,
        truncated: discovered_more_files,
    };
    for path in files.into_iter().take(bounds.file_limit) {
        if result.bytes_scanned >= bounds.max_scan_bytes
            || result.matches.len() >= bounds.max_matches
        {
            result.truncated = true;
            break;
        }
        let remaining = bounds.max_scan_bytes - result.bytes_scanned;
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .map_err(|error| partial_io_error(&path, error))?
            .take(remaining)
            .read_to_end(&mut bytes)
            .map_err(|error| partial_io_error(&path, error))?;
        result.files_scanned += 1;
        result.bytes_scanned = result.bytes_scanned.saturating_add(bytes.len() as u64);
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains(query) {
                continue;
            }
            let start = index.saturating_sub(bounds.context_lines);
            let end = (index + bounds.context_lines + 1).min(lines.len());
            result.matches.push(SearchMatch {
                path: path.clone(),
                line_number: index + 1,
                context: (start..end)
                    .map(|context_index| ContextLine {
                        line_number: context_index + 1,
                        text: lines[context_index].to_string(),
                    })
                    .collect(),
            });
            if result.matches.len() >= bounds.max_matches {
                result.truncated = true;
                break;
            }
        }
    }
    Ok(result)
}

fn collect_regular_files(root: &Path, limit: usize) -> Result<Vec<PathBuf>, PartialExecutionError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| partial_io_error(root, error))?;
    if metadata.file_type().is_symlink() {
        return Err(partial_error(
            "MTS_PARTIAL_UNSUPPORTED_RESOURCE",
            "Bounded search does not follow symbolic links.",
        ));
    }
    if metadata.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !metadata.is_dir() {
        return Err(partial_error(
            "MTS_PARTIAL_UNSUPPORTED_RESOURCE",
            "Bounded search requires a regular file or directory.",
        ));
    }
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| partial_io_error(&directory, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| partial_io_error(&directory, error))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let file_type = entry
                .file_type()
                .map_err(|error| partial_io_error(&entry.path(), error))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
                if files.len() > limit {
                    return Ok(files);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorBounds {
    pub max_matches: usize,
    pub before: usize,
    pub after: usize,
}

impl Default for ErrorBounds {
    fn default() -> Self {
        Self {
            max_matches: 100,
            before: 3,
            after: 8,
        }
    }
}

pub fn extract_error_regions(
    input: &str,
    bounds: ErrorBounds,
) -> Result<BoundedText, PartialExecutionError> {
    if input.len() as u64 > MAX_REPLACEMENT_SCAN_BYTES {
        return Err(partial_error(
            "MTS_PARTIAL_INPUT_TOO_LARGE",
            "Error extraction input exceeds the 16 MiB safety limit.",
        ));
    }
    if bounds.max_matches == 0 {
        return Err(partial_error(
            "MTS_PARTIAL_INVALID_BOUNDS",
            "The error match limit must be positive.",
        ));
    }
    let lines: Vec<&str> = input.lines().collect();
    let mut selected = BTreeSet::new();
    let mut matches = 0;
    for (index, line) in lines.iter().enumerate() {
        if !is_error_line(line) {
            continue;
        }
        matches += 1;
        let start = index.saturating_sub(bounds.before);
        let end = (index + bounds.after + 1).min(lines.len());
        selected.extend(start..end);
        if matches >= bounds.max_matches {
            break;
        }
    }
    let mut text = String::new();
    let mut previous = None;
    for index in selected.iter().copied() {
        if previous.is_some_and(|previous_index| index > previous_index + 1) {
            text.push_str("... omitted ...\n");
        }
        text.push_str(lines[index]);
        text.push('\n');
        previous = Some(index);
    }
    let returned_bytes = text.len() as u64;
    let original_bytes = input.len() as u64;
    Ok(BoundedText {
        text,
        original_bytes,
        returned_bytes,
        omitted_bytes: original_bytes.saturating_sub(returned_bytes),
        truncated: selected.len() < lines.len(),
        start_line: selected.first().map(|index| index + 1).unwrap_or(0),
        end_line: selected.last().map(|index| index + 1).unwrap_or(0),
    })
}

fn is_error_line(line: &str) -> bool {
    let lowercase = line.to_ascii_lowercase();
    [
        "error",
        "failed",
        "failure",
        "fatal",
        "panic",
        "traceback",
        "exception",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceMetadata {
    pub kind: ResourceKind,
    pub bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub content_type: Option<String>,
    pub truncated: bool,
}

pub fn bounded_metadata(
    path: &Path,
    max_entries: usize,
) -> Result<ResourceMetadata, PartialExecutionError> {
    if max_entries == 0 {
        return Err(partial_error(
            "MTS_PARTIAL_INVALID_BOUNDS",
            "The metadata entry limit must be positive.",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| partial_io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(partial_error(
            "MTS_PARTIAL_UNSUPPORTED_RESOURCE",
            "Metadata collection does not follow symbolic links.",
        ));
    }
    if metadata.is_file() {
        return Ok(ResourceMetadata {
            kind: ResourceKind::File,
            bytes: metadata.len(),
            file_count: 1,
            directory_count: 0,
            content_type: path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase()),
            truncated: false,
        });
    }
    if !metadata.is_dir() {
        return Err(partial_error(
            "MTS_PARTIAL_UNSUPPORTED_RESOURCE",
            "Metadata collection requires a regular file or directory.",
        ));
    }
    let mut result = ResourceMetadata {
        kind: ResourceKind::Directory,
        bytes: 0,
        file_count: 0,
        directory_count: 1,
        content_type: None,
        truncated: false,
    };
    let mut stack = vec![path.to_path_buf()];
    let mut visited = 0usize;
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| partial_io_error(&directory, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| partial_io_error(&directory, error))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            visited += 1;
            if visited > max_entries {
                result.truncated = true;
                return Ok(result);
            }
            let file_type = entry
                .file_type()
                .map_err(|error| partial_io_error(&entry.path(), error))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                result.directory_count = result.directory_count.saturating_add(1);
                stack.push(entry.path());
            } else if file_type.is_file() {
                result.file_count = result.file_count.saturating_add(1);
                result.bytes = result.bytes.saturating_add(
                    entry
                        .metadata()
                        .map_err(|error| partial_io_error(&entry.path(), error))?
                        .len(),
                );
            }
        }
    }
    Ok(result)
}

fn partial_error(code: &'static str, message: &str) -> PartialExecutionError {
    PartialExecutionError {
        code,
        message: message.to_string(),
    }
}

fn partial_io_error(path: &Path, error: io::Error) -> PartialExecutionError {
    PartialExecutionError {
        code: "MTS_PARTIAL_IO_ERROR",
        message: format!("Could not access '{}': {error}.", path.display()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileUpdate {
    pub path: PathBuf,
    pub contents: Vec<u8>,
    pub expected_original_hash: Option<String>,
}

impl FileUpdate {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
            expected_original_hash: None,
        }
    }

    pub fn expecting_hash(mut self, hash: impl Into<String>) -> Self {
        self.expected_original_hash = Some(hash.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationResult {
    Passed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitResult {
    Pending,
    Committed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackResult {
    NotNeeded,
    RolledBack,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTransactionJournal {
    pub path: PathBuf,
    pub original_hash: Option<String>,
    pub candidate_hash: String,
    pub validation_result: ValidationResult,
    pub commit_result: CommitResult,
    pub rollback_result: RollbackResult,
    pub drift_detected_before_commit: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransactionJournal {
    pub files: Vec<FileTransactionJournal>,
}

#[derive(Debug)]
pub struct TransactionError {
    pub code: &'static str,
    pub message: String,
    pub journal: TransactionJournal,
}

impl fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TransactionError {}

pub struct FanoutTransaction;

impl FanoutTransaction {
    pub fn commit<F>(
        updates: Vec<FileUpdate>,
        validator: F,
    ) -> Result<TransactionJournal, TransactionError>
    where
        F: Fn(&Path, &[u8]) -> Result<(), String>,
    {
        run_transaction(updates, validator, None)
    }
}

#[derive(Debug)]
struct StagedFile {
    target: PathBuf,
    temporary: PathBuf,
    backup: PathBuf,
    original_existed: bool,
    original_moved: bool,
    committed: bool,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn run_transaction<F>(
    updates: Vec<FileUpdate>,
    validator: F,
    fail_after: Option<usize>,
) -> Result<TransactionJournal, TransactionError>
where
    F: Fn(&Path, &[u8]) -> Result<(), String>,
{
    if updates.is_empty() {
        return Ok(TransactionJournal::default());
    }
    let mut journal = TransactionJournal::default();
    let mut staged = Vec::new();
    let mut unique_paths = HashSet::new();

    for update in updates {
        let target = absolute_lexical_path(&update.path).map_err(|error| TransactionError {
            code: "MTS_TRANSACTION_PREPARE_FAILED",
            message: format!("Could not resolve '{}': {error}.", update.path.display()),
            journal: journal.clone(),
        })?;
        if !unique_paths.insert(target.clone()) {
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_TRANSACTION_PREPARE_FAILED",
                message: format!("Target '{}' is listed more than once.", target.display()),
                journal,
            });
        }
        let parent = target.parent().ok_or_else(|| TransactionError {
            code: "MTS_TRANSACTION_PREPARE_FAILED",
            message: format!("Target '{}' has no parent directory.", target.display()),
            journal: journal.clone(),
        })?;
        if !parent.is_dir() {
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_TRANSACTION_PREPARE_FAILED",
                message: format!("Parent directory '{}' does not exist.", parent.display()),
                journal,
            });
        }
        let original = match fs::read(&target) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                cleanup_staged(&staged);
                return Err(TransactionError {
                    code: "MTS_TRANSACTION_PREPARE_FAILED",
                    message: format!("Could not read '{}': {error}.", target.display()),
                    journal,
                });
            }
        };
        let original_hash = original.as_deref().map(content_hash);
        let drift = update
            .expected_original_hash
            .as_ref()
            .is_some_and(|expected| Some(expected) != original_hash.as_ref());
        let mut entry = FileTransactionJournal {
            path: target.clone(),
            original_hash,
            candidate_hash: content_hash(&update.contents),
            validation_result: ValidationResult::Passed,
            commit_result: CommitResult::Pending,
            rollback_result: RollbackResult::NotNeeded,
            drift_detected_before_commit: drift,
        };
        if drift {
            entry.validation_result = ValidationResult::Failed;
            journal.files.push(entry);
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_INSTALL_CONFIG_CHANGED",
                message: format!(
                    "Target '{}' changed before commit; no files were changed.",
                    target.display()
                ),
                journal,
            });
        }
        if let Err(message) = validator(&target, &update.contents) {
            entry.validation_result = ValidationResult::Failed;
            journal.files.push(entry);
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_TRANSACTION_VALIDATION_FAILED",
                message: format!("Candidate for '{}' is invalid: {message}", target.display()),
                journal,
            });
        }
        let suffix = TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let stem = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("mts-policy");
        let temporary = parent.join(format!(".{stem}.mts-tmp-{}-{suffix}", std::process::id()));
        let backup = parent.join(format!(".{stem}.mts-bak-{}-{suffix}", std::process::id()));
        if let Err(error) = write_staged(&temporary, &update.contents) {
            journal.files.push(entry);
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_TRANSACTION_PREPARE_FAILED",
                message: format!("Could not stage '{}': {error}.", target.display()),
                journal,
            });
        }
        journal.files.push(entry);
        staged.push(StagedFile {
            target,
            temporary,
            backup,
            original_existed: original.is_some(),
            original_moved: false,
            committed: false,
        });
    }

    for (index, file) in staged.iter().enumerate() {
        let current_hash = match fs::read(&file.target) {
            Ok(contents) => Some(content_hash(&contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                cleanup_staged(&staged);
                return Err(TransactionError {
                    code: "MTS_TRANSACTION_PREPARE_FAILED",
                    message: format!(
                        "Could not recheck '{}' before commit: {error}.",
                        file.target.display()
                    ),
                    journal,
                });
            }
        };
        if current_hash != journal.files[index].original_hash {
            journal.files[index].drift_detected_before_commit = true;
            journal.files[index].validation_result = ValidationResult::Failed;
            cleanup_staged(&staged);
            return Err(TransactionError {
                code: "MTS_INSTALL_CONFIG_CHANGED",
                message: format!(
                    "Target '{}' changed while candidates were staged; no files were changed.",
                    file.target.display()
                ),
                journal,
            });
        }
    }

    for index in 0..staged.len() {
        if fail_after == Some(index) {
            journal.files[index].commit_result = CommitResult::Failed;
            return rollback_after_error(staged, journal, "Injected transaction failure.");
        }
        let file = &mut staged[index];
        let commit_result = (|| -> io::Result<()> {
            if file.original_existed {
                fs::rename(&file.target, &file.backup)?;
                file.original_moved = true;
            }
            fs::rename(&file.temporary, &file.target)?;
            file.committed = true;
            sync_parent(file.target.parent().expect("prepared target has a parent"))?;
            Ok(())
        })();
        match commit_result {
            Ok(()) => journal.files[index].commit_result = CommitResult::Committed,
            Err(error) => {
                journal.files[index].commit_result = CommitResult::Failed;
                return rollback_after_error(staged, journal, &error.to_string());
            }
        }
    }

    for file in &staged {
        if file.backup.exists() {
            let _ = fs::remove_file(&file.backup);
        }
    }
    Ok(journal)
}

fn rollback_after_error(
    mut staged: Vec<StagedFile>,
    mut journal: TransactionJournal,
    cause: &str,
) -> Result<TransactionJournal, TransactionError> {
    let mut rollback_failed = false;
    for (index, file) in staged.iter_mut().enumerate().rev() {
        if file.committed && file.target.exists() && fs::remove_file(&file.target).is_err() {
            rollback_failed = true;
        }
        if file.original_moved {
            if fs::rename(&file.backup, &file.target).is_err() {
                rollback_failed = true;
                journal.files[index].rollback_result = RollbackResult::Failed;
            } else {
                journal.files[index].rollback_result = RollbackResult::RolledBack;
            }
        } else if file.committed {
            journal.files[index].rollback_result = if rollback_failed {
                RollbackResult::Failed
            } else {
                RollbackResult::RolledBack
            };
        }
        let _ = fs::remove_file(&file.temporary);
        let _ = fs::remove_file(&file.backup);
    }
    Err(TransactionError {
        code: if rollback_failed {
            "MTS_TRANSACTION_ROLLBACK_FAILED"
        } else {
            "MTS_TRANSACTION_COMMIT_FAILED"
        },
        message: if rollback_failed {
            format!("Commit failed and at least one original file could not be restored: {cause}")
        } else {
            format!("Commit failed; all changed files were restored: {cause}")
        },
        journal,
    })
}

fn write_staged(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn cleanup_staged(staged: &[StagedFile]) {
    for file in staged {
        let _ = fs::remove_file(&file.temporary);
        let _ = fs::remove_file(&file.backup);
    }
}

fn absolute_lexical_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

pub fn content_hash(contents: &[u8]) -> String {
    format!("fnv1a64:{:016x}", fnv1a64(contents))
}

fn fnv1a64(contents: &[u8]) -> u64 {
    contents.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn intent(resource: &str) -> ResourceIntentKey {
        ResourceIntentKey::new(
            "Codex-CLI",
            "session-1",
            "workspace-1",
            IntentClass::ExactRead,
            resource,
            Operation::Read,
            "mts-partial-1",
            None,
            true,
        )
    }

    fn temp_directory(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after the Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("mts-core-{name}-{}-{unique}", std::process::id()));
        fs::create_dir(&path).expect("temporary directory must be created");
        path
    }

    #[test]
    fn parses_and_decides_with_deterministic_precedence() {
        let full = CompiledPolicy::parse_full(
            "@MTS-FULL-1 node_modules/** | write,edit | Dependencies are immutable\n",
        )
        .unwrap();
        let partial = CompiledPolicy::parse_partial(
            "node_modules/**/*.d.ts | read | limit | max_lines=200,max_bytes=65536 | Return bounded declarations\n\
             **/*.log | read | errors-only | max_matches=100,before=3,after=8 | Return error regions only\n",
        )
        .unwrap();
        let mut policies = PolicySet::new();
        policies.add(full, RuleScope::Harness);
        policies.add(partial, RuleScope::Harness);

        assert!(matches!(
            policies.decide(Operation::Edit, "/work/node_modules/pkg/index.js"),
            Decision::FullBlock(FullBlockDecision { rule_id, .. }) if rule_id == "MTS-FULL-1"
        ));
        assert!(matches!(
            policies.decide(Operation::Read, "C:\\work\\node_modules\\pkg\\types.d.ts"),
            Decision::PartialBlock(PartialBlockDecision {
                substitute: SubstituteMetadata {
                    mode: ReplacementMode::Limit,
                    original_executed: false,
                    ..
                },
                ..
            })
        ));
        assert_eq!(
            policies.decide(Operation::Unknown, "node_modules/pkg"),
            Decision::Allow
        );
    }

    #[test]
    fn parser_is_atomic_and_reports_duplicates() {
        let duplicate =
            "node_modules/** | read | First reason\nnode_modules/** | read | Second reason\n";
        let parsed = CompiledPolicy::parse_full(duplicate).unwrap();
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.warnings.len(), 1);

        let invalid = "node_modules/** | read | Good\n**/*.log | unknown | Invalid\n";
        let error = CompiledPolicy::parse_full(invalid).unwrap_err();
        assert_eq!(error.line, 2);
        assert_eq!(error.code, "MTS_POLICY_PARSE_ERROR");
    }

    #[test]
    fn glob_and_resource_normalization_cover_common_aliases() {
        assert!(glob_matches("**/*.log", "build.log"));
        assert!(glob_matches(
            "node_modules/**",
            "/repo/node_modules/pkg/index.js"
        ));
        assert!(!glob_matches("node_modules/**", "/repo/src/index.js"));
        assert!(glob_matches_with_case(
            "NODE_MODULES/**",
            "node_modules/pkg",
            true
        ));
        assert!(!glob_matches_with_case(
            "NODE_MODULES/**",
            "node_modules/pkg",
            false
        ));
        assert_eq!(
            intent("C:\\Repo\\node_modules\\pkg\\..\\pkg\\index.js"),
            intent("c:/repo/node_modules/pkg/index.js")
        );
        assert_eq!(normalize_resource("a/b/../c", false), "a/c");
    }

    #[test]
    fn retry_circuit_opens_on_the_third_equivalent_attempt_and_resets_for_progress() {
        let key = intent("node_modules/pkg/index.js");
        let mut circuit = RetryCircuit::default();
        assert_eq!(
            circuit
                .record_attempt(
                    key.clone(),
                    Some("node_modules/pkg/index.js"),
                    10,
                    RetryProgress::None
                )
                .state,
            RetryState::BlockedWithGuidance
        );
        assert_eq!(
            circuit
                .record_attempt(
                    key.clone(),
                    Some("./node_modules/pkg/index.js"),
                    11,
                    RetryProgress::None
                )
                .state,
            RetryState::SubstituteReturned
        );
        let third = circuit.record_attempt(key.clone(), None, 12, RetryProgress::None);
        assert_eq!(third.state, RetryState::CircuitOpen);
        assert_eq!(
            third.compact_code.as_deref(),
            Some("MTS_CIRCUIT_OPEN:MTS-PARTIAL-1")
        );
        assert_eq!(
            circuit
                .record_attempt(key, None, 13, RetryProgress::SmallerBounds)
                .state,
            RetryState::BlockedWithGuidance
        );
    }

    #[test]
    fn shell_extraction_is_literal_and_does_not_execute_search_data() {
        let intents = extract_shell_intents(
            "rg 'cat node_modules' README.md; cat node_modules/pkg/index.js",
            ShellFamily::Unix,
        )
        .unwrap();
        assert_eq!(intents.len(), 2);
        assert_eq!(intents[0].operation, Operation::Search);
        assert_eq!(intents[0].resource, "README.md");
        assert_eq!(intents[0].search_query.as_deref(), Some("cat node_modules"));
        assert_eq!(intents[1].operation, Operation::Read);
        assert_eq!(intents[1].resource, "node_modules/pkg/index.js");
        let bounded = extract_shell_intents("sed -n 1,200p source.rs", ShellFamily::Unix).unwrap();
        assert_eq!(bounded[0].line_range, Some((1, 200)));

        let powershell = extract_shell_intents(
            "Get-Content node_modules\\pkg\\index.js",
            ShellFamily::PowerShell,
        )
        .unwrap();
        assert_eq!(powershell[0].intent_class, IntentClass::ExactRead);
        assert!(extract_shell_intents("cat $(find .)", ShellFamily::Unix).is_err());
    }

    #[test]
    fn bounded_partial_operations_return_transparent_limits() {
        let directory = temp_directory("partial");
        let first = directory.join("first.log");
        let second = directory.join("second.txt");
        fs::write(&first, "one\nERROR: broken\nthree\nfour\n").unwrap();
        fs::write(&second, "needle here\nordinary\n").unwrap();

        let read = bounded_read(
            &first,
            ReadBounds {
                max_lines: 2,
                max_bytes: 64,
                start_line: 2,
                end_line: None,
            },
        )
        .unwrap();
        assert_eq!(read.text, "ERROR: broken\nthree\n");
        assert!(read.truncated);

        let search = bounded_search(
            &directory,
            "needle",
            SearchBounds {
                max_matches: 1,
                context_lines: 0,
                file_limit: 10,
                max_scan_bytes: 1_024,
            },
        )
        .unwrap();
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].path, second);

        let errors = extract_error_regions(
            "one\nERROR: broken\nthree\nfour",
            ErrorBounds {
                max_matches: 1,
                before: 0,
                after: 1,
            },
        )
        .unwrap();
        assert_eq!(errors.text, "ERROR: broken\nthree\n");
        let metadata = bounded_metadata(&directory, 10).unwrap();
        assert_eq!(metadata.file_count, 2);
        assert!(!metadata.truncated);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn savings_never_go_negative_and_first_saving_is_deduplicated() {
        let key = intent("node_modules/pkg/index.js");
        let input = SavingsInput {
            protected_bytes: 2_000_000,
            avoided_output_bytes: 1_000,
            replacement_output_bytes: 1_200,
            retry_overhead_bytes: 50,
            bytes_per_token: 4,
            estimate_method: "byte-range-v1".to_string(),
            confidence: EstimateConfidence::Low,
        };
        let mut ledger = SavingsLedger::default();
        let first = ledger.account(key.clone(), input.clone(), 0);
        let repeat = ledger.account(key, input, 500);
        assert_eq!(first.net_avoided_bytes, 0);
        assert_eq!(first.estimated_net_tokens_saved, 0);
        assert!(repeat.deduplicated);
        assert_eq!(repeat.first_prevention_saving, 0);
    }

    #[test]
    fn fanout_commit_and_rollback_preserve_all_targets() {
        let directory = temp_directory("transaction");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, b"old-first").unwrap();
        fs::write(&second, b"old-second").unwrap();
        let updates = vec![
            FileUpdate::new(&first, b"new-first".to_vec())
                .expecting_hash(content_hash(b"old-first")),
            FileUpdate::new(&second, b"new-second".to_vec())
                .expecting_hash(content_hash(b"old-second")),
        ];

        let error = run_transaction(updates.clone(), |_, _| Ok(()), Some(1)).unwrap_err();
        assert_eq!(error.code, "MTS_TRANSACTION_COMMIT_FAILED");
        assert_eq!(fs::read(&first).unwrap(), b"old-first");
        assert_eq!(fs::read(&second).unwrap(), b"old-second");

        let journal = FanoutTransaction::commit(updates, |_, contents| {
            (!contents.is_empty())
                .then_some(())
                .ok_or_else(|| "Policy content must not be empty.".to_string())
        })
        .unwrap();
        assert!(journal
            .files
            .iter()
            .all(|entry| entry.commit_result == CommitResult::Committed));
        assert_eq!(fs::read(&first).unwrap(), b"new-first");
        assert_eq!(fs::read(&second).unwrap(), b"new-second");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fanout_rejects_drift_before_any_write() {
        let directory = temp_directory("drift");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let updates = vec![
            FileUpdate::new(&first, b"changed".to_vec()).expecting_hash(content_hash(b"first")),
            FileUpdate::new(&second, b"changed".to_vec()).expecting_hash("fnv1a64:wrong"),
        ];
        let error = FanoutTransaction::commit(updates, |_, _| Ok(())).unwrap_err();
        assert_eq!(error.code, "MTS_INSTALL_CONFIG_CHANGED");
        assert_eq!(fs::read(&first).unwrap(), b"first");
        assert_eq!(fs::read(&second).unwrap(), b"second");
        fs::remove_dir_all(directory).unwrap();
    }
}
