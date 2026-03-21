use std::borrow::Cow;
use std::hint::cold_path;
use std::sync::LazyLock;

use serde::Deserialize;
use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::comments::{Comment, CommentKind, Comments};
#[cfg(not(test))]
use swc_core::common::errors::HANDLER;
use swc_core::common::plugin::metadata::TransformPluginMetadataContextKind;
use swc_core::common::source_map::PLACEHOLDER_SP;
use swc_core::common::{BytePos, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{CallExpr, Expr, Lit, Program, Tpl};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use swc_core::plugin::plugin_transform;
use swc_core::plugin::proxies::{PluginCommentsProxy, TransformPluginProgramMetadata};

#[derive(Deserialize)]
struct PluginConfig {
    dirs: Option<Vec<String>>,
}

/// Default source directories used to locate the package boundary in import
/// paths: `src`, `lib`, `dist`.
static DEFAULT_DIRS: [&str; 3] = ["src", "lib", "dist"];

static WEBPACK_MODE_ATOM: LazyLock<Atom> =
    LazyLock::new(|| Atom::from(r#" webpackMode: "lazy-once" "#));

/// UTF-8 import/template text as `Cow::Borrowed` when `value` is valid UTF-8 (usual case),
/// otherwise uses the same lossy conversion as [`Wtf8Atom::to_string_lossy`].
#[inline]
fn wtf8_atom_to_cow(s: &Wtf8Atom) -> Cow<'_, str> {
    match s.as_str() {
        Some(t) => Cow::Borrowed(t),
        None => {
            cold_path();
            s.to_string_lossy()
        },
    }
}

#[inline]
fn is_source_dir(dirs: Option<&[String]>, seg: &str) -> bool {
    match dirs {
        Some(custom) => custom.iter().any(|d| d == seg),
        None => DEFAULT_DIRS.contains(&seg),
    }
}

pub struct TransformVisitor<C: Comments> {
    comments: C,
    filename: Option<String>,
    source_dirs: Option<Box<[String]>>,
}

impl<C: Comments> TransformVisitor<C> {
    /// Strips any existing `webpackChunkName` / `webpackMode` block comments at `pos`,
    /// preserves all other leading comments, then prepends fresh `webpackChunkName` and
    /// `webpackMode: "lazy-once"` comments so webpack picks up the generated name.
    #[inline]
    fn replace_chunk_comments(&self, pos: BytePos, chunk_name: &str) {
        let mut preserved = self.comments.take_leading(pos).unwrap_or_default();
        preserved.retain(|c| {
            !(c.kind == CommentKind::Block
                && (c.text.contains("webpackChunkName") || c.text.contains("webpackMode")))
        });

        if !preserved.is_empty() {
            self.comments.add_leading_comments(pos, preserved);
        }

        self.comments.add_leading(
            pos,
            Comment { kind: CommentKind::Block, span: DUMMY_SP, text: WEBPACK_MODE_ATOM.clone() },
        );
        self.comments.add_leading(
            pos,
            Comment {
                kind: CommentKind::Block,
                span: DUMMY_SP,
                text: {
                    let mut t = String::with_capacity(23 + chunk_name.len());
                    t.push_str(r#" webpackChunkName: ""#);
                    t.push_str(chunk_name);
                    t.push_str(r#"" "#);
                    Atom::from(t)
                },
            },
        );
    }
}

impl<C: Comments> VisitMut for TransformVisitor<C> {
    fn visit_mut_call_expr(&mut self, node: &mut CallExpr) {
        node.visit_mut_children_with(self);

        if !node.callee.is_import() && !is_fallback_import_call(node) {
            return;
        }

        let Some(arg) = node.args.first() else {
            cold_path();
            return;
        };

        let (import_path, pos): (Cow<'_, str>, _) = match &*arg.expr {
            Expr::Lit(Lit::Str(str_lit)) => {
                let anchor = if str_lit.span == DUMMY_SP || str_lit.span == PLACEHOLDER_SP {
                    arg.expr.span_lo()
                } else {
                    str_lit.span.lo()
                };
                (wtf8_atom_to_cow(&str_lit.value), anchor)
            },
            Expr::Tpl(tpl) => match flatten_tpl(tpl) {
                Some(path) => {
                    let anchor = if tpl.span == DUMMY_SP || tpl.span == PLACEHOLDER_SP {
                        arg.expr.span_lo()
                    } else {
                        tpl.span.lo()
                    };
                    (Cow::Owned(path), anchor)
                },
                None => {
                    cold_path();
                    emit_warn(tpl.span, "could not flatten template literal");
                    return;
                },
            },
            _ => {
                cold_path();
                emit_warn(arg.expr.span(), "unsupported import argument type");
                return;
            },
        };

        let Some(chunk_name) = generate_chunk_name(
            &import_path,
            self.filename.as_deref(),
            self.source_dirs.as_deref(),
        ) else {
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
#[inline]
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
#[inline]
fn generate_chunk_name(
    import_path: &str,
    filename: Option<&str>,
    source_dirs: Option<&[String]>,
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

    Some(format_chunk_name(&prefix, leaf, trailing_slash))
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
#[inline]
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
    let leaf = rest.map(|r| r.rsplit('/').next().unwrap_or(r));

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
#[inline]
fn parse_bare_import<'a>(
    path: &'a str,
    source_dirs: Option<&[String]>,
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

    let leaf = rest.map(|r| r.rsplit('/').next().unwrap_or(r));

    let prefix = if let Some(rest_str) = rest {
        let mut prev: Option<&str> = Some(pkg);
        let mut best: Option<&str> = None;
        for seg in rest_str.split('/') {
            if is_source_dir(source_dirs, seg) {
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
#[inline]
fn chunk_name_relative(
    path: &str,
    filename: &str,
    source_dirs: Option<&[String]>,
) -> Option<String> {
    let resolved = resolve_relative_path(filename, path);
    let Some(pkg) = extract_package_from_path(&resolved, source_dirs) else {
        cold_path();
        return None;
    };
    let leaf = strip_extension(path.rsplit('/').next().unwrap_or(path));
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
#[inline]
fn format_chunk_name(prefix: &str, leaf: Option<&str>, trailing_slash: bool) -> String {
    match leaf {
        Some(l) => {
            let stripped = strip_extension(l);
            let mut out = String::with_capacity(prefix.len() + 1 + stripped.len());
            write_camel_case(prefix, &mut out);
            out.push('.');
            out.push_str(stripped);
            out
        },
        None if trailing_slash => {
            let mut out = String::with_capacity(prefix.len() + 6);
            write_camel_case(prefix, &mut out);
            out.push_str(".index");
            out
        },
        None => to_camel_case(prefix),
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
#[inline]
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
#[inline]
fn to_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    write_camel_case(s, &mut out);
    out
}

/// Strips the file extension (last `.xxx`) from a filename.
///
/// Returns everything before the last `.`, or the whole string if there is no
/// dot or the dot is at position 0 (hidden files like `.gitignore`).
#[inline]
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
#[inline]
fn resolve_relative_path(filename: &str, import_path: &str) -> String {
    let dir = filename.rsplit_once('/').map_or("", |(d, _)| d);
    let mut segments: Vec<&str> = Vec::with_capacity(32);
    let mut total_len = 0usize;

    for seg in dir.split('/').filter(|s| !s.is_empty()) {
        if segments.is_empty() {
            total_len = seg.len();
        } else {
            total_len += 1 + seg.len();
        }
        segments.push(seg);
    }

    for part in import_path.split('/') {
        match part {
            "." | "" => {},
            ".." => {
                if let Some(popped) = segments.pop() {
                    if segments.is_empty() {
                        total_len = 0;
                    } else {
                        total_len = total_len.saturating_sub(1 + popped.len());
                    }
                }
            },
            other => {
                if segments.is_empty() {
                    total_len = other.len();
                } else {
                    total_len += 1 + other.len();
                }
                segments.push(other);
            },
        }
    }

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
#[inline]
fn extract_package_from_path<'a>(
    resolved: &'a str,
    source_dirs: Option<&[String]>,
) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    let mut second_to_last: Option<&str> = None;
    let mut last: Option<&str> = None;

    for seg in resolved.split('/').filter(|s| !s.is_empty()) {
        if is_source_dir(source_dirs, seg) {
            best = last;
        }
        second_to_last = last;
        last = Some(seg);
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
#[inline]
fn flatten_tpl(tpl: &Tpl) -> Option<String> {
    let capacity: usize =
        tpl.quasis.iter().map(|q| q.cooked.as_ref().map_or(0, |c| c.len())).sum::<usize>()
            + tpl.exprs.len() * 7;
    let mut result = String::with_capacity(capacity);

    let mut quasis = tpl.quasis.iter();
    let Some(first) = quasis.next() else {
        cold_path();
        return None;
    };
    let Some(cooked) = first.cooked.as_ref() else {
        cold_path();
        return None;
    };
    result.push_str(wtf8_atom_to_cow(cooked).as_ref());

    for (quasi, _expr) in quasis.zip(tpl.exprs.iter()) {
        result.push_str("dynamic");
        let Some(cooked) = quasi.cooked.as_ref() else {
            cold_path();
            return None;
        };
        result.push_str(wtf8_atom_to_cow(cooked).as_ref());
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
fn parse_plugin_config(raw_config: Option<String>) -> Option<Box<[String]>> {
    let Some(raw) = raw_config else {
        cold_path();
        emit_warn(DUMMY_SP, "plugin config is missing; using default dirs");
        return None;
    };

    match serde_json::from_str::<PluginConfig>(&raw) {
        Ok(config) => config.dirs.filter(|v| !v.is_empty()).map(Vec::into_boxed_slice),
        Err(_) => {
            cold_path();
            emit_warn(DUMMY_SP, "invalid plugin config; using default dirs");
            None
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
    let source_dirs = parse_plugin_config(metadata.get_transform_plugin_config());

    let filename = metadata.get_context(&TransformPluginMetadataContextKind::Filename).map(|f| {
        match f.as_bytes().first() {
            Some(&b'/') => f,
            _ => resolve_filename(f, &metadata),
        }
    });

    if let Some(comments) = metadata.comments {
        program.visit_mut_with(&mut TransformVisitor::<PluginCommentsProxy> {
            comments,
            filename,
            source_dirs,
        });
    } else {
        cold_path();
        emit_warn(DUMMY_SP, "comments proxy unavailable; plugin has no effect");
    }

    program
}

#[cfg(test)]
mod tests;
