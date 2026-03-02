#![feature(cold_path)]

use std::borrow::Cow;
use std::hint::cold_path;

use serde::Deserialize;
use swc_core::atoms::Atom;
use swc_core::common::comments::{Comment, CommentKind, Comments};
#[cfg(not(test))]
use swc_core::common::errors::HANDLER;
use swc_core::common::plugin::metadata::TransformPluginMetadataContextKind;
use swc_core::common::{BytePos, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{CallExpr, Expr, Lit, Program, Tpl};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use swc_core::plugin::plugin_transform;
use swc_core::plugin::proxies::{PluginCommentsProxy, TransformPluginProgramMetadata};

#[derive(Deserialize)]
struct PluginConfig {
    #[serde(default = "default_dirs")]
    dirs: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self { dirs: default_dirs() }
    }
}

/// Default source directories used to locate the package boundary in import
/// paths: `src`, `lib`, `dist`.
#[cold]
fn default_dirs() -> Vec<String> {
    vec!["src".to_string(), "lib".to_string(), "dist".to_string()]
}

pub struct TransformVisitor {
    comments: PluginCommentsProxy,
    filename: Option<String>,
    source_dirs: Vec<String>,
}

impl TransformVisitor {
    /// Strips any existing `webpackChunkName` / `webpackMode` block comments at `pos`,
    /// preserves all other leading comments, then prepends fresh `webpackChunkName` and
    /// `webpackMode: "lazy-once"` comments so webpack picks up the generated name.
    #[inline(always)]
    fn replace_chunk_comments(&self, pos: BytePos, chunk_name: &str) {
        let preserved: Vec<Comment> = self
            .comments
            .take_leading(pos)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| {
                !(c.kind == CommentKind::Block
                    && (c.text.contains("webpackChunkName") || c.text.contains("webpackMode")))
            })
            .collect();

        if !preserved.is_empty() {
            self.comments.add_leading_comments(pos, preserved);
        }

        self.comments.add_leading(
            pos,
            Comment {
                kind: CommentKind::Block,
                span: DUMMY_SP,
                text: Atom::from(" webpackMode: \"lazy-once\" "),
            },
        );
        self.comments.add_leading(
            pos,
            Comment {
                kind: CommentKind::Block,
                span: DUMMY_SP,
                text: {
                    let mut t = String::with_capacity(23 + chunk_name.len());
                    t.push_str(" webpackChunkName: \"");
                    t.push_str(chunk_name);
                    t.push_str("\" ");
                    Atom::from(t)
                },
            },
        );
    }
}

impl VisitMut for TransformVisitor {
    fn visit_mut_call_expr(&mut self, node: &mut CallExpr) {
        node.visit_mut_children_with(self);

        let is_native_import = node.callee.is_import();
        let is_fallback_import = is_fallback_import_call(node);

        if !is_native_import && !is_fallback_import {
            return;
        }

        let Some(arg) = node.args.first() else {
            cold_path();
            return;
        };

        let (import_path, pos): (Cow<'_, str>, _) = match &*arg.expr {
            Expr::Lit(Lit::Str(str_lit)) => (str_lit.value.to_string_lossy(), arg.expr.span_lo()),
            Expr::Tpl(tpl) => match flatten_tpl(tpl) {
                Some(path) => (Cow::Owned(path), arg.expr.span_lo()),
                None => {
                    cold_path();
                    if is_native_import {
                        emit_warn(tpl.span, "could not flatten template literal");
                    }
                    return;
                },
            },
            _ => {
                cold_path();
                if is_native_import {
                    emit_warn(arg.expr.span(), "unsupported import argument type");
                }
                return;
            },
        };

        let Some(chunk_name) =
            generate_chunk_name(&import_path, self.filename.as_deref(), &self.source_dirs)
        else {
            cold_path();
            emit_warn(arg.expr.span(), "could not generate chunk name");
            return;
        };

        self.replace_chunk_comments(pos, &chunk_name);
    }
}

/// Returns `true` when the callee is a plain `import(...)` identifier -- a
/// fallback pattern some bundlers emit when the native `import()` keyword has
/// been down-leveled to a function call.
#[inline(always)]
fn is_fallback_import_call(node: &CallExpr) -> bool {
    node.callee.as_expr().is_some_and(|expr| expr.as_ident().is_some_and(|id| id.sym == "import"))
}

// ---------------------------------------------------------------------------
// Chunk-name generation
// ---------------------------------------------------------------------------

/// Produces a deterministic webpack chunk name from an import path.
///
/// The naming convention is `camelCasePrefix.leafSegment` where *prefix*
/// identifies the package and *leaf* is the last path segment (extension
/// stripped). Three import shapes are handled:
///
/// | Shape | Example | Result |
/// |---|---|---|
/// | Scoped | `@scope/pkg/sub/Leaf` | `scopePkg.Leaf` |
/// | Relative | `./components/Button` | `mfWidgets.Button` |
/// | Bare | `lodash/fp/get` | `lodash.get` |
///
/// A trailing slash signals an index re-export and appends `.index`
/// (e.g. `lodash/` -> `lodash.index`). Absolute paths (`/...`) are rejected.
fn generate_chunk_name(
    import_path: &str,
    filename: Option<&str>,
    source_dirs: &[String],
) -> Option<String> {
    let first = match import_path.as_bytes().first() {
        Some(&b) => b,
        None => {
            cold_path();
            return None;
        },
    };

    if first == b'/' {
        cold_path();
        return None;
    }

    let (path, trailing_slash) = match import_path.strip_suffix('/') {
        Some(stripped) => (stripped, true),
        None => (import_path, false),
    };

    if first == b'.' {
        if let Some(filename) = filename {
            return chunk_name_relative(path, filename, source_dirs);
        } else {
            cold_path();
            return None;
        }
    }

    let Some((prefix, leaf)) = (match first {
        b'@' if path.as_bytes().get(1) == Some(&b'/') => {
            cold_path();
            parse_bare_import(&path[2..], source_dirs)
        },
        b'@' => parse_scoped_import(path),
        _ => parse_bare_import(path, source_dirs),
    }) else {
        cold_path();
        return None;
    };

    format_chunk_name(&prefix, leaf, trailing_slash)
}

/// Parses a scoped npm import `@scope/pkg[/deep/path/Leaf]`.
///
/// Splits on `/` into at most three parts via `splitn(3, '/')`:
///   `scope`, `pkg`, and an optional `rest`.
/// The prefix is formed by joining scope and pkg with `-` (e.g. `scope-a-ui`),
/// which `to_camel_case` later turns into `scopeAUi`. If `rest` is present the
/// leaf is its last `/`-separated segment; otherwise there is no leaf.
///
/// Returns `(prefix, Option<leaf>)` as `(Cow::Owned, Option<&str>)`.
#[inline(always)]
fn parse_scoped_import(path: &str) -> Option<(Cow<'_, str>, Option<&str>)> {
    debug_assert!(path.starts_with('@'));
    let mut parts = path[1..].splitn(3, '/');
    let Some(scope) = parts.next().filter(|s| !s.is_empty()) else {
        cold_path();
        return None;
    };
    let Some(pkg) = parts.next().filter(|s| !s.is_empty()) else {
        cold_path();
        return None;
    };
    let rest = parts.next();

    let mut joined = String::with_capacity(scope.len() + 1 + pkg.len());
    joined.push_str(scope);
    joined.push('-');
    joined.push_str(pkg);
    let prefix = Cow::Owned(joined);
    let leaf = rest.and_then(|r| r.rsplit('/').next());

    Some((prefix, leaf))
}

/// Parses a bare (non-scoped, non-relative) import like `lodash/fp/get`.
///
/// When `source_dirs` is non-empty the segments are scanned left-to-right for
/// a known source directory (`src`, `lib`, `dist`, …). The segment immediately
/// *before* the rightmost match becomes the prefix (package name). If no
/// source dir is found the first `/`-delimited segment is used as fallback.
///
/// The leaf is always the last path segment (extension stripped later).
///
/// Returns `(prefix, Option<leaf>)` where prefix is `Cow::Borrowed` to avoid
/// allocating when the package name is already a slice of the input.
#[inline(always)]
fn parse_bare_import<'a>(
    path: &'a str,
    source_dirs: &[String],
) -> Option<(Cow<'a, str>, Option<&'a str>)> {
    let (pkg, rest) = match path.find('/') {
        Some(pos) => (&path[..pos], Some(&path[pos + 1..])),
        None => {
            cold_path();
            (path, None)
        },
    };

    if pkg.is_empty() {
        cold_path();
        return None;
    }

    let leaf = rest.and_then(|r| r.rsplit('/').next());

    let prefix = if let Some(rest_str) = rest {
        let mut prev: Option<&str> = Some(pkg);
        let mut best: Option<&str> = None;
        for seg in rest_str.split('/') {
            if source_dirs.iter().any(|d| d.as_str() == seg) {
                best = prev;
            }
            prev = Some(seg);
        }
        match best {
            Some(b) => Cow::Borrowed(b),
            None => Cow::Borrowed(pkg),
        }
    } else {
        Cow::Borrowed(pkg)
    };

    Some((prefix, leaf))
}

/// Handles relative imports (`./foo` or `../bar`).
///
/// Pipeline:
/// 1. Resolve the import against the current file's directory to get an
///    absolute-ish path.
/// 2. Walk that resolved path to find which package owns it (via
///    `extract_package_from_path`).
/// 3. Take the last segment of the *original* import as the leaf.
/// 4. Format as `camelCase(package).stripExt(leaf)`.
fn chunk_name_relative(path: &str, filename: &str, source_dirs: &[String]) -> Option<String> {
    let resolved = resolve_relative_path(filename, path);
    let Some(pkg) = extract_package_from_path(&resolved, source_dirs) else {
        cold_path();
        return None;
    };
    let leaf = strip_extension(unsafe { path.rsplit('/').next().unwrap_unchecked() });
    let mut out = String::with_capacity(pkg.len() + 1 + leaf.len());
    write_camel_case(pkg, &mut out);
    out.push('.');
    out.push_str(leaf);
    Some(out)
}

/// Formats the final chunk name from a camelCased prefix and an optional leaf.
///
/// - `Some(leaf)` -> `"camelPrefix.strippedLeaf"`
/// - `None` + trailing slash -> `"camelPrefix.index"` (index re-export convention)
/// - `None` -> `"camelPrefix"` (bare package import)
#[inline(always)]
fn format_chunk_name(prefix: &str, leaf: Option<&str>, trailing_slash: bool) -> Option<String> {
    match leaf {
        Some(l) => {
            let stripped = strip_extension(l);
            let mut out = String::with_capacity(prefix.len() + 1 + stripped.len());
            write_camel_case(prefix, &mut out);
            out.push('.');
            out.push_str(stripped);
            Some(out)
        },
        None if trailing_slash => {
            let mut out = String::with_capacity(prefix.len() + 6);
            write_camel_case(prefix, &mut out);
            out.push_str(".index");
            Some(out)
        },
        None => Some(to_camel_case(prefix)),
    }
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Appends the camelCase form of a kebab-case / snake_case ASCII string to `out`.
///
/// Scans bytes left-to-right: `-` and `_` are consumed as "capitalize next"
/// signals. The following byte (if any) is uppercased; everything else is
/// copied verbatim.
///
/// Writes directly into the caller's buffer to avoid an intermediate `String`
/// allocation. **Safety invariant**: the input must be ASCII (true for npm
/// package names); the output therefore stays valid UTF-8.
#[inline(always)]
fn write_camel_case(s: &str, out: &mut String) {
    out.reserve(s.len());
    // SAFETY: every byte pushed is ASCII (uppercase or verbatim copy of an
    // ASCII input), so the String's UTF-8 invariant is preserved.
    let buf = unsafe { out.as_mut_vec() };
    let mut cap_next = false;

    for &b in s.as_bytes() {
        if b == b'-' || b == b'_' {
            cap_next = true;
        } else if cap_next {
            buf.push(b.to_ascii_uppercase());
            cap_next = false;
        } else {
            buf.push(b);
        }
    }
}

/// Convenience wrapper: returns a new camelCase `String`.
fn to_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    write_camel_case(s, &mut out);
    out
}

/// Strips the file extension (last `.xxx`) from a filename.
///
/// Returns everything before the last `.`, or the whole string if there is no
/// dot or the dot is at position 0 (hidden files like `.gitignore`).
#[inline(always)]
fn strip_extension(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(pos) if pos > 0 => &filename[..pos],
        _ => filename,
    }
}

/// Resolves a relative import path against the directory of the importing file.
///
/// Uses a segment-stack approach: splits the file's directory into segments,
/// then replays each part of the import path -- `.` and empty segments are
/// skipped, `..` pops the stack, anything else is pushed. The final segments
/// are joined with `/`.
fn resolve_relative_path(filename: &str, import_path: &str) -> String {
    let dir = filename.rsplit_once('/').map_or("", |(d, _)| d);
    let mut segments: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();

    for part in import_path.split('/') {
        match part {
            "." | "" => {},
            ".." => {
                segments.pop();
            },
            other => segments.push(other),
        }
    }

    let total_len: usize =
        segments.iter().map(|s| s.len()).sum::<usize>() + segments.len().saturating_sub(1);
    let mut result = String::with_capacity(total_len);
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(seg);
    }
    result
}

/// Identifies which package owns a resolved path by scanning for a known
/// source directory (`src`, `lib`, `dist`, or custom).
///
/// Walks segments left-to-right, recording the segment *before* each source-dir
/// match. Because the scan overwrites on every match, the *rightmost* (innermost)
/// source-dir wins -- this correctly handles nested `node_modules` trees like
/// `node_modules/@a/pkg1/node_modules/@b/pkg2/src/...`.
///
/// Fallback when no source-dir is found: return the second-to-last segment
/// (the "parent directory" heuristic), or the only segment if the path has
/// just one.
fn extract_package_from_path<'a>(resolved: &'a str, source_dirs: &[String]) -> Option<&'a str> {
    let mut prev: Option<&str> = None;
    let mut best: Option<&str> = None;
    let mut second_to_last: Option<&str> = None;
    let mut last: Option<&str> = None;

    for seg in resolved.split('/').filter(|s| !s.is_empty()) {
        if source_dirs.iter().any(|d| d.as_str() == seg) {
            best = prev;
        }
        second_to_last = last;
        last = Some(seg);
        prev = Some(seg);
    }

    best.or(second_to_last).or(last)
}

// ---------------------------------------------------------------------------
// Template-literal flattening
// ---------------------------------------------------------------------------

/// Flattens a template literal into a plain path string by interleaving
/// quasis (static text) and a fixed `"dynamic"` placeholder for each
/// expression.
///
/// For example, `` `./locale/${language}` `` becomes `"./locale/dynamic"`.
///
/// Returns `None` if any quasi is missing its cooked value or the result is
/// empty (e.g. a template with only empty strings and no expressions).
fn flatten_tpl(tpl: &Tpl) -> Option<String> {
    let capacity: usize =
        tpl.quasis.iter().map(|q| q.raw.len()).sum::<usize>() + tpl.exprs.len() * 7;
    let mut result = String::with_capacity(capacity);

    for (i, quasi) in tpl.quasis.iter().enumerate() {
        let Some(cooked) = quasi.cooked.as_ref() else {
            cold_path();
            return None;
        };
        result.push_str(&cooked.to_string_lossy());
        if i < tpl.exprs.len() {
            result.push_str("dynamic");
        }
    }

    if result.is_empty() {
        cold_path();
        return None;
    }

    Some(result)
}

// ---------------------------------------------------------------------------
// Plugin entry point & config
// ---------------------------------------------------------------------------

/// Deserializes the plugin configuration from JSON, falling back to defaults
/// on missing or invalid input.
fn parse_plugin_config(raw_config: Option<String>) -> PluginConfig {
    let Some(raw) = raw_config else {
        cold_path();
        emit_warn(DUMMY_SP, "plugin config is missing; using default dirs");
        return PluginConfig::default();
    };

    match serde_json::from_str::<PluginConfig>(&raw) {
        Ok(config) => config,
        Err(_) => {
            cold_path();
            emit_warn(DUMMY_SP, "invalid plugin config; using default dirs");
            PluginConfig::default()
        },
    }
}

/// Emits a compiler warning via the SWC diagnostic handler.
/// In test mode this is a no-op.
#[inline(never)]
#[cold]
fn emit_warn(span: Span, message: &str) {
    #[cfg(not(test))]
    HANDLER.with(|handler| {
        if span == DUMMY_SP {
            handler.warn(message);
        } else {
            handler.span_warn(span, message);
        }
    });

    #[cfg(test)]
    {
        let _ = (span, message);
    }
}

/// Resolves a relative filename to an absolute path by prepending the cwd
/// obtained from the SWC metadata context. Only called when the filename
/// lacks a leading `/`, so this is marked `#[cold]`.
#[cold]
#[inline(never)]
fn resolve_filename(filename: String, metadata: &TransformPluginProgramMetadata) -> String {
    match metadata.get_context(&TransformPluginMetadataContextKind::Cwd) {
        Some(c) => {
            let cwd = match c.as_bytes().last() {
                Some(&b'/') => &c[..c.len() - 1],
                _ => &c,
            };
            let mut abs = String::with_capacity(cwd.len() + 1 + filename.len());
            abs.push_str(cwd);
            abs.push('/');
            abs.push_str(&filename);
            abs
        },
        None => filename,
    }
}

/// Plugin entry point. Parses configuration, resolves the current filename
/// to an absolute path when necessary, and runs the transform visitor over
/// the program AST to inject `webpackChunkName` comments.
#[plugin_transform]
pub fn process_transform(
    mut program: Program,
    metadata: TransformPluginProgramMetadata,
) -> Program {
    let config = parse_plugin_config(metadata.get_transform_plugin_config());

    let filename = metadata.get_context(&TransformPluginMetadataContextKind::Filename).map(|f| {
        match f.as_bytes().first() {
            Some(&b'/') => f,
            _ => resolve_filename(f, &metadata),
        }
    });

    if let Some(comments) = metadata.comments {
        program.visit_mut_with(&mut TransformVisitor {
            comments,
            filename,
            source_dirs: config.dirs,
        });
    } else {
        cold_path();
        emit_warn(DUMMY_SP, "comments proxy unavailable; plugin has no effect");
    }

    program
}

#[cfg(test)]
mod tests {
    use swc_core::common::SyntaxContext;
    use swc_core::ecma::ast::{ExprOrSpread, Ident, TplElement};

    use super::*;

    fn make_tpl(quasis_strs: &[&str], expr_names: &[&str]) -> Tpl {
        let quasis = quasis_strs
            .iter()
            .enumerate()
            .map(|(i, s)| TplElement {
                span: DUMMY_SP,
                tail: i == quasis_strs.len() - 1,
                cooked: Some((*s).into()),
                raw: Atom::from(*s),
            })
            .collect();
        let exprs = expr_names
            .iter()
            .map(|name| {
                Box::new(Expr::Ident(Ident {
                    span: DUMMY_SP,
                    ctxt: SyntaxContext::empty(),
                    sym: Atom::from(*name),
                    optional: false,
                })) as Box<Expr>
            })
            .collect();
        Tpl { span: DUMMY_SP, quasis, exprs }
    }

    fn source_dirs() -> Vec<String> {
        default_dirs()
    }

    // --- to_camel_case ---

    #[test]
    fn camel_case_kebab() {
        assert_eq!(to_camel_case("package-name"), "packageName");
    }

    #[test]
    fn camel_case_snake() {
        assert_eq!(to_camel_case("package_name"), "packageName");
    }

    #[test]
    fn camel_case_mixed() {
        assert_eq!(to_camel_case("my-cool_lib"), "myCoolLib");
    }

    #[test]
    fn camel_case_no_separators() {
        assert_eq!(to_camel_case("lodash"), "lodash");
    }

    // --- strip_extension ---

    #[test]
    fn strip_ts_extension() {
        assert_eq!(strip_extension("Component.tsx"), "Component");
        assert_eq!(strip_extension("utils.ts"), "utils");
        assert_eq!(strip_extension("index.js"), "index");
        assert_eq!(strip_extension("helper.mjs"), "helper");
    }

    #[test]
    fn strip_no_extension() {
        assert_eq!(strip_extension("ComponentName"), "ComponentName");
    }

    #[test]
    fn strip_any_extension() {
        assert_eq!(strip_extension("data.json"), "data");
        assert_eq!(strip_extension("style.css"), "style");
        assert_eq!(strip_extension("template.vue"), "template");
    }

    #[test]
    fn strip_hidden_file_unchanged() {
        assert_eq!(strip_extension(".hidden"), ".hidden");
    }

    // --- resolve_relative_path ---

    #[test]
    fn resolve_relative_current_dir() {
        let resolved = resolve_relative_path("/project/src/components/App.tsx", "./Button");
        assert_eq!(resolved, "project/src/components/Button");
    }

    #[test]
    fn resolve_relative_parent_dir() {
        let resolved = resolve_relative_path("/project/src/components/App.tsx", "../utils/helpers");
        assert_eq!(resolved, "project/src/utils/helpers");
    }

    #[test]
    fn resolve_deeply_nested() {
        let resolved =
            resolve_relative_path("/project/src/deep/nested/File.tsx", "../../other/Thing");
        assert_eq!(resolved, "project/src/other/Thing");
    }

    // --- extract_package_from_path ---

    #[test]
    fn extract_package_before_src() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("project/mf-widgets/src/components/Button", &dirs);
        assert_eq!(pkg, Some("mf-widgets"));
    }

    #[test]
    fn extract_package_before_lib() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("packages/my-lib/lib/utils", &dirs);
        assert_eq!(pkg, Some("my-lib"));
    }

    #[test]
    fn extract_package_fallback() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("foo/bar/baz", &dirs);
        assert_eq!(pkg, Some("bar"));
    }

    #[test]
    fn extract_package_right_to_left() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path(
            "project/src/packages/mf-widgets/src/components/Button",
            &dirs,
        );
        assert_eq!(pkg, Some("mf-widgets"));
    }

    // --- generate_chunk_name ---

    #[test]
    fn chunk_name_scoped_package() {
        let dirs = source_dirs();
        let name =
            generate_chunk_name("@my-company/package-name/src/my/test/ComponentName", None, &dirs);
        assert_eq!(name, Some("myCompanyPackageName.ComponentName".to_string()));
    }

    #[test]
    fn chunk_name_scoped_with_extension() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@scope/my-lib/utils/helpers.ts", None, &dirs);
        assert_eq!(name, Some("scopeMyLib.helpers".to_string()));
    }

    #[test]
    fn chunk_name_scoped_distinct_scopes() {
        let dirs = source_dirs();
        let scope_a = generate_chunk_name("@scope-a/ui/Button.tsx", None, &dirs);
        let scope_b = generate_chunk_name("@scope-b/ui/Button.tsx", None, &dirs);
        assert_eq!(scope_a, Some("scopeAUi.Button".to_string()));
        assert_eq!(scope_b, Some("scopeBUi.Button".to_string()));
        assert_ne!(scope_a, scope_b);
    }

    #[test]
    fn chunk_name_non_scoped() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash/fp/get", None, &dirs);
        assert_eq!(name, Some("lodash.get".to_string()));
    }

    #[test]
    fn chunk_name_dotted_bare_and_deep_import_collide() {
        let dirs = source_dirs();
        let dotted = generate_chunk_name("lodash.get", None, &dirs);
        let deep = generate_chunk_name("lodash/get", None, &dirs);
        assert_eq!(dotted, Some("lodash.get".to_string()));
        assert_eq!(deep, Some("lodash.get".to_string()));
        assert_eq!(dotted, deep);
    }

    #[test]
    fn chunk_name_bare_deep_path_with_source_dir() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lib/lib/test-package/src/Component", None, &dirs);
        assert_eq!(name, Some("testPackage.Component".to_string()));
    }

    #[test]
    fn chunk_name_relative_with_filename() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./components/MyWidget",
            Some("/project/mf-widgets/src/App.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("mfWidgets.MyWidget".to_string()));
    }

    #[test]
    fn chunk_name_relative_no_filename() {
        let dirs = source_dirs();
        let name = generate_chunk_name("./components/MyWidget", None, &dirs);
        assert_eq!(name, None);
    }

    #[test]
    fn chunk_name_relative_with_extension() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./Button.tsx",
            Some("/project/mf-widgets/src/components/App.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("mfWidgets.Button".to_string()));
    }

    #[test]
    fn chunk_name_relative_filename_with_cwd_matches_absolute() {
        let dirs = source_dirs();
        let abs = generate_chunk_name(
            "./components/MyWidget",
            Some("/project/mf-widgets/src/App.tsx"),
            &dirs,
        );
        let rel_with_cwd = generate_chunk_name(
            "./components/MyWidget",
            Some("/project/mf-widgets/src/App.tsx"),
            &dirs,
        );
        assert_eq!(abs, rel_with_cwd);

        let cwd_joined = {
            let c = "/project";
            let f = "mf-widgets/src/App.tsx";
            let mut s = String::with_capacity(c.len() + 1 + f.len());
            s.push_str(c);
            s.push('/');
            s.push_str(f);
            s
        };
        let from_joined = generate_chunk_name("./components/MyWidget", Some(&cwd_joined), &dirs);
        assert_eq!(abs, from_joined);
    }

    #[test]
    fn chunk_name_relative_filename_cwd_trailing_slash() {
        let dirs = source_dirs();
        let c = "/project/";
        let f = "mf-widgets/src/App.tsx";
        let c_trimmed = c.strip_suffix('/').unwrap_or(c);
        let mut joined = String::with_capacity(c_trimmed.len() + 1 + f.len());
        joined.push_str(c_trimmed);
        joined.push('/');
        joined.push_str(f);

        let name = generate_chunk_name("./components/MyWidget", Some(&joined), &dirs);
        assert_eq!(name, Some("mfWidgets.MyWidget".to_string()));
    }

    #[test]
    fn chunk_name_absolute_leading_slash_returns_none() {
        let dirs = source_dirs();
        let name =
            generate_chunk_name("/project/mf-widgets/src/components/Button.tsx", None, &dirs);
        assert_eq!(name, None);
    }

    // --- parse_plugin_config ---

    #[test]
    fn parse_plugin_config_missing_uses_defaults() {
        let config = parse_plugin_config(None);
        assert_eq!(config.dirs, default_dirs());
    }

    #[test]
    fn parse_plugin_config_invalid_uses_defaults() {
        let config = parse_plugin_config(Some("{invalid".to_string()));
        assert_eq!(config.dirs, default_dirs());
    }

    #[test]
    fn parse_plugin_config_valid_uses_source_dirs() {
        let config = parse_plugin_config(Some("{\"dirs\":[\"client\",\"shared\"]}".to_string()));
        assert_eq!(config.dirs, vec!["client".to_string(), "shared".to_string()]);
    }

    // --- is_fallback_import_call ---

    fn make_ident_expr(name: &str) -> Box<Expr> {
        Box::new(Expr::Ident(Ident {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            sym: Atom::from(name),
            optional: false,
        }))
    }

    #[test]
    fn fallback_import_ident_call_is_supported() {
        let call = CallExpr {
            span: DUMMY_SP,
            callee: swc_core::ecma::ast::Callee::Expr(make_ident_expr("import")),
            args: vec![ExprOrSpread {
                spread: None,
                expr: Box::new(Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: "./module".into(),
                    raw: None,
                }))),
            }],
            type_args: None,
            ctxt: SyntaxContext::empty(),
        };
        assert!(is_fallback_import_call(&call));
    }

    #[test]
    fn non_import_ident_call_is_not_fallback_import() {
        let call = CallExpr {
            span: DUMMY_SP,
            callee: swc_core::ecma::ast::Callee::Expr(make_ident_expr("load")),
            args: vec![],
            type_args: None,
            ctxt: SyntaxContext::empty(),
        };
        assert!(!is_fallback_import_call(&call));
    }

    // --- flatten_tpl ---

    #[test]
    fn flatten_tpl_single_expr() {
        let tpl = make_tpl(&["./locale/", ""], &["language"]);
        assert_eq!(flatten_tpl(&tpl), Some("./locale/dynamic".to_string()));
    }

    #[test]
    fn flatten_tpl_multiple_exprs() {
        let tpl = make_tpl(&["./", "/", ""], &["scope", "name"]);
        assert_eq!(flatten_tpl(&tpl), Some("./dynamic/dynamic".to_string()));
    }

    #[test]
    fn flatten_tpl_no_exprs() {
        let tpl = make_tpl(&["./static/path"], &[]);
        assert_eq!(flatten_tpl(&tpl), Some("./static/path".to_string()));
    }

    #[test]
    fn flatten_tpl_generates_chunk_name() {
        let dirs = source_dirs();
        let path = flatten_tpl(&make_tpl(&["./locale/", ""], &["language"])).unwrap();
        let name = generate_chunk_name(&path, Some("/project/mf-widgets/src/App.tsx"), &dirs);
        assert_eq!(name, Some("mfWidgets.dynamic".to_string()));
    }

    // --- generate_chunk_name: bare packages ---

    #[test]
    fn chunk_name_bare_non_scoped() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash", None, &dirs);
        assert_eq!(name, Some("lodash".to_string()));
    }

    #[test]
    fn chunk_name_bare_non_scoped_with_separator() {
        let dirs = source_dirs();
        let name = generate_chunk_name("my-package", None, &dirs);
        assert_eq!(name, Some("myPackage".to_string()));
    }

    #[test]
    fn chunk_name_bare_scoped() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@scope/pkg", None, &dirs);
        assert_eq!(name, Some("scopePkg".to_string()));
    }

    #[test]
    fn chunk_name_bare_scoped_with_separators() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@my-scope/my-pkg", None, &dirs);
        assert_eq!(name, Some("myScopeMyPkg".to_string()));
    }

    #[test]
    fn chunk_name_scoped_trailing_slash_returns_none() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@scope/", None, &dirs);
        assert_eq!(name, None);
    }

    #[test]
    fn chunk_name_empty_returns_none() {
        let dirs = source_dirs();
        let name = generate_chunk_name("", None, &dirs);
        assert_eq!(name, None);
    }

    // --- to_camel_case: edge cases ---

    #[test]
    fn camel_case_leading_separator() {
        assert_eq!(to_camel_case("-foo"), "Foo");
    }

    #[test]
    fn camel_case_trailing_separator() {
        assert_eq!(to_camel_case("foo-"), "foo");
    }

    #[test]
    fn camel_case_consecutive_separators() {
        assert_eq!(to_camel_case("a--b"), "aB");
    }

    #[test]
    fn camel_case_empty() {
        assert_eq!(to_camel_case(""), "");
    }

    // --- strip_extension: edge cases ---

    #[test]
    fn strip_double_extension() {
        assert_eq!(strip_extension("file.test.tsx"), "file.test");
    }

    #[test]
    fn strip_extension_dot_only() {
        assert_eq!(strip_extension("."), ".");
    }

    #[test]
    fn strip_extension_empty() {
        assert_eq!(strip_extension(""), "");
    }

    // --- resolve_relative_path: edge cases ---

    #[test]
    fn resolve_relative_bare_filename() {
        let resolved = resolve_relative_path("App.tsx", "./components/Button");
        assert_eq!(resolved, "components/Button");
    }

    #[test]
    fn resolve_relative_excessive_parent_traversal() {
        let resolved = resolve_relative_path("/a/b/File.tsx", "../../../x");
        assert_eq!(resolved, "x");
    }

    // --- extract_package_from_path: edge cases ---

    #[test]
    fn extract_package_single_segment() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("onlyone", &dirs);
        assert_eq!(pkg, Some("onlyone"));
    }

    #[test]
    fn extract_package_empty_path() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("", &dirs);
        assert_eq!(pkg, None);
    }

    // --- flatten_tpl: edge cases ---

    #[test]
    fn flatten_tpl_empty_quasis() {
        let tpl = make_tpl(&[""], &[]);
        assert_eq!(flatten_tpl(&tpl), None);
    }

    #[test]
    fn flatten_tpl_only_expr() {
        let tpl = make_tpl(&["", ""], &["name"]);
        assert_eq!(flatten_tpl(&tpl), Some("dynamic".to_string()));
    }

    // --- generate_chunk_name: nested paths ---

    #[test]
    fn chunk_name_nested_components_via_lib() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./components/GroupName",
            Some("/project/my-app/lib/components/Group/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("myApp.GroupName".to_string()));
    }

    #[test]
    fn chunk_name_nested_components_via_src() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./components/GroupName",
            Some("/project/mf-widgets/src/components/Group/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("mfWidgets.GroupName".to_string()));
    }

    #[test]
    fn chunk_name_nested_parent_traversal_into_sibling_group() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "../GroupB/components/GroupBChild",
            Some("/project/mf-widgets/src/components/GroupA/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("mfWidgets.GroupBChild".to_string()));
    }

    #[test]
    fn chunk_name_deeply_nested_upward_traversal() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "../../GroupTitle",
            Some("/project/mf-widgets/src/components/Group/components/GroupName/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("mfWidgets.GroupTitle".to_string()));
    }

    #[test]
    fn extract_package_nested_components_finds_innermost_source_dir() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path(
            "project/my-app/lib/components/Group/components/GroupName",
            &dirs,
        );
        assert_eq!(pkg, Some("my-app"));
    }

    // --- generate_chunk_name: node_modules ---

    #[test]
    fn extract_package_nested_node_modules_with_src() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path(
            "node_modules/@scope1/pkg1/node_modules/@scope2/pkg2/src/components/Button",
            &dirs,
        );
        assert_eq!(pkg, Some("pkg2"));
    }

    #[test]
    fn extract_package_single_node_modules_with_lib() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("node_modules/@scope/package/lib/utils/helper", &dirs);
        assert_eq!(pkg, Some("package"));
    }

    #[test]
    fn chunk_name_relative_import_inside_nested_node_modules() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./components/Button",
            Some("/project/node_modules/@scope1/pkg1/node_modules/@scope2/pkg2/src/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("pkg2.Button".to_string()));
    }

    #[test]
    fn chunk_name_relative_import_inside_single_node_modules() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "../utils/formatDate",
            Some("/project/node_modules/@company/ui-kit/src/components/DatePicker.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("uiKit.formatDate".to_string()));
    }

    #[test]
    fn chunk_name_relative_import_unscoped_node_modules() {
        let dirs = source_dirs();
        let name = generate_chunk_name(
            "./locale/en",
            Some("/project/node_modules/some-lib/src/index.tsx"),
            &dirs,
        );
        assert_eq!(name, Some("someLib.en".to_string()));
    }

    // --- generate_chunk_name: trailing slash ---

    #[test]
    fn chunk_name_non_scoped_trailing_slash() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash/", None, &dirs);
        assert_eq!(name, Some("lodash.index".to_string()));
    }

    #[test]
    fn chunk_name_non_scoped_deep_trailing_slash() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash/fp/", None, &dirs);
        assert_eq!(name, Some("lodash.fp".to_string()));
    }

    #[test]
    fn chunk_name_scoped_deep_trailing_slash() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@scope/pkg/sub/", None, &dirs);
        assert_eq!(name, Some("scopePkg.sub".to_string()));
    }

    #[test]
    fn chunk_name_relative_trailing_slash() {
        let dirs = source_dirs();
        let name =
            generate_chunk_name("./components/", Some("/project/mf-widgets/src/App.tsx"), &dirs);
        assert_eq!(name, Some("mfWidgets.components".to_string()));
    }

    // --- generate_chunk_name: scoped edge cases ---

    #[test]
    fn chunk_name_at_sign_only_returns_none() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@scope", None, &dirs);
        assert_eq!(name, None);
    }

    #[test]
    fn chunk_name_at_alias_with_leaf() {
        let dirs = source_dirs();
        assert_eq!(
            generate_chunk_name("@/components/Button", None, &dirs),
            Some("components.Button".to_string())
        );
    }

    #[test]
    fn chunk_name_at_alias_deep_path() {
        let dirs = source_dirs();
        assert_eq!(
            generate_chunk_name("@/utils/helpers/format", None, &dirs),
            Some("utils.format".to_string())
        );
    }

    #[test]
    fn chunk_name_at_alias_bare() {
        let dirs = source_dirs();
        assert_eq!(generate_chunk_name("@/lib", None, &dirs), Some("lib".to_string()));
    }

    #[test]
    fn chunk_name_at_alias_trailing_slash() {
        let dirs = source_dirs();
        assert_eq!(generate_chunk_name("@/lib/", None, &dirs), Some("lib.index".to_string()));
    }

    #[test]
    fn chunk_name_non_scoped_with_index() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash/index", None, &dirs);
        assert_eq!(name, Some("lodash.index".to_string()));
    }

    // --- generate_chunk_name: source-dir aware ---

    #[test]
    fn chunk_name_scoped_with_and_without_source_dir_same() {
        let dirs = source_dirs();
        let with_src =
            generate_chunk_name("@my-company/package-name/src/my/test/ComponentName", None, &dirs);
        let without_src =
            generate_chunk_name("@my-company/package-name/my/test/ComponentName", None, &dirs);
        assert_eq!(with_src, Some("myCompanyPackageName.ComponentName".to_string()));
        assert_eq!(with_src, without_src);
    }

    #[test]
    fn chunk_name_at_alias_with_source_dir() {
        let dirs = source_dirs();
        let name = generate_chunk_name("@/lib/test-package/src/Component", None, &dirs);
        assert_eq!(name, Some("testPackage.Component".to_string()));
    }

    #[test]
    fn chunk_name_bare_without_source_dir_unchanged() {
        let dirs = source_dirs();
        let name = generate_chunk_name("lodash/fp/get", None, &dirs);
        assert_eq!(name, Some("lodash.get".to_string()));
    }

    // --- extract_package_from_path: source-dir at root ---

    #[test]
    fn extract_package_source_dir_at_root_uses_fallback() {
        let dirs = source_dirs();
        let pkg = extract_package_from_path("src/components/Button", &dirs);
        assert_eq!(pkg, Some("components"));
    }

    // --- to_camel_case: digits & degenerate ---

    #[test]
    fn camel_case_only_separators() {
        assert_eq!(to_camel_case("---"), "");
    }

    #[test]
    fn camel_case_with_digits() {
        assert_eq!(to_camel_case("lib-2"), "lib2");
    }

    // --- flatten_tpl: non-ident expressions ---

    #[test]
    fn flatten_tpl_numeric_expr_uses_dynamic() {
        let tpl = Tpl {
            span: DUMMY_SP,
            quasis: vec![
                TplElement {
                    span: DUMMY_SP,
                    tail: false,
                    cooked: Some("./v".into()),
                    raw: Atom::from("./v"),
                },
                TplElement {
                    span: DUMMY_SP,
                    tail: true,
                    cooked: Some("/api".into()),
                    raw: Atom::from("/api"),
                },
            ],
            exprs: vec![Box::new(Expr::Lit(Lit::Num(swc_core::ecma::ast::Number {
                span: DUMMY_SP,
                value: 2.0,
                raw: None,
            })))],
        };
        assert_eq!(flatten_tpl(&tpl), Some("./vdynamic/api".to_string()));
    }

    #[test]
    fn flatten_tpl_any_expr_uses_dynamic() {
        let tpl = Tpl {
            span: DUMMY_SP,
            quasis: vec![
                TplElement {
                    span: DUMMY_SP,
                    tail: false,
                    cooked: Some("./path/".into()),
                    raw: Atom::from("./path/"),
                },
                TplElement {
                    span: DUMMY_SP,
                    tail: true,
                    cooked: Some("".into()),
                    raw: Atom::from(""),
                },
            ],
            exprs: vec![Box::new(Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
                span: DUMMY_SP,
                value: true,
            })))],
        };
        assert_eq!(flatten_tpl(&tpl), Some("./path/dynamic".to_string()));
    }

    // --- parse_plugin_config: edge cases ---

    #[test]
    fn parse_plugin_config_empty_object_uses_defaults() {
        let config = parse_plugin_config(Some("{}".to_string()));
        assert_eq!(config.dirs, default_dirs());
    }

    #[test]
    fn parse_plugin_config_extra_fields_ignored() {
        let config =
            parse_plugin_config(Some("{\"dirs\":[\"client\"],\"unknown\":true}".to_string()));
        assert_eq!(config.dirs, vec!["client".to_string()]);
    }

    // --- generate_chunk_name: trailing-slash disambiguation ---

    #[test]
    fn chunk_name_punycode_builtin_vs_npm() {
        let dirs = source_dirs();
        let builtin = generate_chunk_name("punycode", None, &dirs);
        let npm = generate_chunk_name("punycode/", None, &dirs);
        assert_eq!(builtin, Some("punycode".to_string()));
        assert_eq!(npm, Some("punycode.index".to_string()));
        assert_ne!(builtin, npm);
    }

    #[test]
    fn chunk_name_scoped_bare_vs_trailing_slash() {
        let dirs = source_dirs();
        let bare = generate_chunk_name("@scope/pkg", None, &dirs);
        let with_slash = generate_chunk_name("@scope/pkg/", None, &dirs);
        assert_eq!(bare, Some("scopePkg".to_string()));
        assert_eq!(with_slash, Some("scopePkg.index".to_string()));
        assert_ne!(bare, with_slash);
    }
}
