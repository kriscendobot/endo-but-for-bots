//! Dependency walk for `endor run <entry.js>` (Phase 5 of
//! `designs/endor-run-expanded.md`).
//!
//! When the entry-point form encounters `import` (or `export ...
//! from`) statements, this module walks the dependency graph from
//! the entry file. Each discovered file becomes a module within
//! its enclosing compartment; each bare specifier (`"lodash"`,
//! `"@scope/pkg"`, `"@scope/pkg/sub"`) resolves to a package in
//! a `node_modules` directory upward from the importing file,
//! becoming its own compartment in the synthesised
//! compartment-map.
//!
//! Phase 4 (`crate::cas_archive::ingest_entry_point`) handles the
//! no-dependency case as a synthesised one-compartment archive.
//! Phase 5 supersedes Phase 4 *when imports are present*; Phase 4
//! remains the fast path for the import-free case (and the
//! verified contract `cas_after_rejected_ingest_is_unchanged`,
//! `ingest_entry_point_run_path_matches_zip_run_path`, etc.,
//! still hold). The CLI dispatch picks between the two: an entry
//! whose static-import scan returns no specifiers is routed to
//! [`crate::cas_archive::ingest_entry_point`]; an entry with one
//! or more importable bare or relative specifiers is routed to
//! [`ingest_entry_point_with_deps`].
//!
//! ### Scope deviations from the design's Option B
//!
//! The design (`designs/endor-run-expanded.md` § Compartment
//! mapper implementation) names an *XS-hosted compartment mapper*
//! bundle as the chosen near-term approach (Option B). That
//! approach requires (a) bundling `@endo/compartment-mapper` for
//! XS execution, (b) wiring filesystem host powers into a fresh
//! mapper machine, (c) running a two-machine handshake to capture
//! the mapper's CompartmentMap output before the execution
//! machine boots. The infrastructure for (a)-(c) is shared with
//! the daemon-side XS bundles and is not yet present in this
//! crate.
//!
//! This module ships the design's Option A (a *Rust-native
//! mapper*) for the Phase 5 acceptance test ("`endor run app.js`
//! where `app.js` imports from a local `node_modules` package").
//! Option A and Option B converge on the same CompartmentMap
//! shape for the cases this module handles (static ES module
//! imports, `node_modules`-resolved bare specifiers, plain
//! `package.json` `main`/`exports.default`/`./index.js`
//! resolution); the design's deviation pattern from Phase 4 is
//! re-applied here. The XS-hosted mapper bundle remains
//! warranted whenever the dependency graph requires features
//! that the Rust-native walk does not implement (conditional
//! exports beyond `default`, dynamic `import()`, CJS-from-CJS
//! `require()` walking, the registry-table path from
//! `designs/endor-npm-registry-proxy.md` Phase 4); those land in
//! follow-up work and the Status section of the design records
//! the deferral.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::cas::{ContentStore, TreeEntry};
use crate::cas_archive::{
    load_archive_from_cas, parser_for_extension as parser_for_ext, IngestedArchive,
    SYNTHETIC_COMPARTMENT_ID,
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Static-import scan result for a single source file.
///
/// Only specifiers reachable via ES-module static syntax
/// (`import ... from "x"`, `export ... from "x"`, side-effecting
/// `import "x"`) are extracted. Dynamic `import("...")` and
/// CommonJS `require("...")` are intentionally out of scope
/// for the Phase 5 walk (Phase 4's deviation note records the
/// XS-hosted mapper as the chosen long-term home for those).
///
/// The deduplication discipline: callers receive each unique
/// specifier once in source-occurrence order, so a downstream
/// emitter that depends on a stable specifier order (the
/// per-compartment `modules` map below, for instance) gets
/// reproducible output for a given source byte sequence.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScannedImports {
    pub specifiers: Vec<String>,
}

/// Walk static imports starting from `entry_path`, ingest every
/// reachable source into the CAS, and return an
/// [`IngestedArchive`] whose `compartment-map.json` describes the
/// full graph.
///
/// Acceptance shape (per the design's Phase 5 test):
///
/// - The entry compartment holds the entry file plus every
///   relative-import-reachable sibling within the same package.
/// - Each `node_modules`-resolved bare specifier becomes its own
///   compartment; its modules and any transitively imported
///   siblings are stored under the package's compartment id
///   (`<pkg-name>-v<version>`, with the version pulled from the
///   package's `package.json` `version` field, falling back to
///   `0.0.0` when absent).
/// - Cross-compartment references are encoded as
///   [`xsnap::archive::ModuleDescriptor::Link`] entries in the
///   importing compartment, so the import hook installed by
///   [`xsnap::archive::install_archive`] routes them via
///   `importNow` on the target compartment.
///
/// Out of scope (returns an `Err` with a descriptive message):
///
/// - A bare specifier that resolves to no `node_modules` tree
///   upward from the importing file.
/// - A package whose `exports`/`main`/`index.js` resolution
///   yields no readable source file.
/// - A subpath import (`"@scope/pkg/lib/foo.js"`) that escapes
///   the resolved package's tree.
///
/// All other failure modes propagate the underlying `io::Error`
/// (a missing source file, a CAS write error, a `package.json`
/// JSON-parse error mapped to `InvalidData`).
pub fn ingest_entry_point_with_deps(
    cas: &ContentStore,
    entry_path: &Path,
) -> io::Result<IngestedArchive> {
    if !entry_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("not a regular file: {}", entry_path.display()),
        ));
    }

    let entry_parser = parser_for_ext(entry_path.extension()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported entry-point extension: {} (expected .js, .mjs, .cjs, .json)",
                entry_path.display()
            ),
        )
    })?;

    let entry_abs = entry_path.canonicalize()?;
    let entry_dir = entry_abs
        .parent()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("entry path has no parent: {}", entry_path.display()),
            )
        })?
        .to_path_buf();

    // The entry compartment is rooted at the entry's directory.
    // For an entry with a sibling `package.json`, the package
    // metadata is read to discover the canonical compartment id
    // (`<name>-v<version>`); otherwise we synthesise one with the
    // Phase 4 placeholder id (`entry-v1.0.0`) so the entry
    // compartment is observably the same shape as Phase 4's
    // synthetic archive.
    let entry_pkg = load_package_metadata(&entry_dir).ok();
    let entry_compartment_id = match &entry_pkg {
        Some(pkg) => compartment_id_for(&pkg.name, &pkg.version),
        None => SYNTHETIC_COMPARTMENT_ID.to_string(),
    };
    let entry_compartment_name = entry_pkg
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "entry".to_string());

    let mut walker = Walker::new(cas);
    walker.add_compartment(
        entry_compartment_id.clone(),
        entry_compartment_name,
        entry_dir.clone(),
    );

    let entry_file_name = entry_abs
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("entry path has no file name: {}", entry_path.display()),
            )
        })?
        .to_string_lossy()
        .into_owned();
    let entry_specifier = format!("./{entry_file_name}");

    walker.enqueue(
        entry_compartment_id.clone(),
        entry_specifier.clone(),
        entry_abs.clone(),
        entry_parser.to_string(),
    );

    walker.drain()?;

    let map_json = walker.emit_map_json(&entry_compartment_id, &entry_specifier);
    let root_hash = walker.write_root_tree(&map_json)?;
    let archive = load_archive_from_cas(cas, &root_hash)?;

    Ok(IngestedArchive { root_hash, archive })
}

/// Scan `source` for ES-module static import specifiers.
///
/// Recognised forms (per the
/// `import-statement-static-syntax` subset of the ECMAScript
/// module grammar):
///
/// - `import "x"`
/// - `import foo from "x"`
/// - `import * as foo from "x"`
/// - `import { a, b as c } from "x"`
/// - `import foo, { a } from "x"`
/// - `export * from "x"`
/// - `export { a } from "x"`
/// - `export * as foo from "x"`
///
/// Both single- and double-quoted specifiers are supported. The
/// scan is character-stream based and intentionally permissive:
/// it skips JS line and block comments and skips over template-
/// literal and regular string literals so a string `"import
/// 'x'"` inside a JS literal is not falsely matched. The
/// downstream walk's resolver rejects anything that isn't a real
/// file, so any remaining false-positive specifier is caught at
/// resolution time rather than producing a malformed compartment
/// map.
pub fn scan_static_imports(source: &str) -> ScannedImports {
    let mut out = ScannedImports::default();
    let mut seen = std::collections::HashSet::new();

    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut at_stmt_start = true;
    while i < bytes.len() {
        let c = bytes[i];

        // Skip whitespace.
        if c.is_ascii_whitespace() {
            if c == b'\n' {
                at_stmt_start = true;
            }
            i += 1;
            continue;
        }

        // Skip line comment.
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Skip block comment.
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            }
            continue;
        }

        // Skip string literal (single, double, or template).
        if c == b'"' || c == b'\'' || c == b'`' {
            let quote = c;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                // For template literals, skip ${ ... } expressions
                // shallowly (don't track nested braces; templates
                // inside templates would slip through, but the
                // import-scan only needs to avoid mis-identifying
                // their contents).
                if quote == b'`' && bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{'
                {
                    let mut depth = 1;
                    i += 2;
                    while i < bytes.len() && depth > 0 {
                        if bytes[i] == b'{' {
                            depth += 1;
                        } else if bytes[i] == b'}' {
                            depth -= 1;
                        }
                        i += 1;
                    }
                    continue;
                }
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            // Resume at the same statement-start state we had on
            // entry; a string literal does not begin a new
            // statement on its own.
            continue;
        }

        // `;` and `}` terminate the current statement; the next
        // token may start a new one.
        if c == b';' || c == b'}' {
            at_stmt_start = true;
            i += 1;
            continue;
        }

        // Only look for `import` / `export` at statement starts.
        // The conservative rule is that `import` and `export`
        // can only begin a statement; an `import.meta` reference
        // appears after an identifier-context dot which we exclude
        // by the `at_stmt_start` gate plus the `import.meta` /
        // `import(` recognition below.
        let is_import = at_stmt_start && matches_keyword(bytes, i, b"import");
        let is_export = at_stmt_start && matches_keyword(bytes, i, b"export");
        if is_import || is_export {
            let keyword_len = if is_import { 6 } else { 7 };
            // Look at the next non-whitespace character to filter
            // out `import.meta` / `import(...)`.
            let mut j = i + keyword_len;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if is_import && j < bytes.len() && (bytes[j] == b'.' || bytes[j] == b'(') {
                // Not a statement we follow.
                i = j + 1;
                at_stmt_start = false;
                continue;
            }
            // Scan forward to the end of the statement (`;` or
            // `\n` outside of nested braces/strings).
            let stmt_end = find_statement_end(bytes, j);
            let body = &source[j..stmt_end];
            // For an `export` statement, only the re-export form
            // (`export ... from "..."` / `export * from "..."`)
            // carries a specifier we follow. A plain `export
            // function ...`, `export const ...`, etc., is a local
            // declaration whose body may contain string literals
            // that have nothing to do with imports.
            if is_export && !contains_from_keyword(body.as_bytes()) {
                i = stmt_end;
                at_stmt_start = false;
                continue;
            }
            // Within the statement, capture the *last* quoted
            // string literal (single- or double-quoted; not
            // template). That is the specifier.
            if let Some(spec) = find_last_string_literal(body) {
                if seen.insert(spec.clone()) {
                    out.specifiers.push(spec);
                }
            }
            i = stmt_end;
            at_stmt_start = false;
            continue;
        }

        // Any other token: the next character is not a statement
        // start.
        at_stmt_start = false;
        i += 1;
    }

    out
}

/// True when `bytes[i..i+kw.len()]` equals `kw` *and* the byte
/// before `i` (if any) is not an identifier continuation and the
/// byte at `i+kw.len()` (if any) is not an identifier continuation.
/// This lets us recognise `import` as a keyword without matching
/// the substring inside `myimport` or `importer`.
fn matches_keyword(bytes: &[u8], i: usize, kw: &[u8]) -> bool {
    if i + kw.len() > bytes.len() {
        return false;
    }
    if &bytes[i..i + kw.len()] != kw {
        return false;
    }
    if i > 0 && is_ident_continue(bytes[i - 1]) {
        return false;
    }
    if i + kw.len() < bytes.len() && is_ident_continue(bytes[i + kw.len()]) {
        return false;
    }
    true
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// True when `bytes` contains the standalone keyword `from`
/// (surrounded by non-identifier characters) outside of any
/// string literal or comment. Used to discriminate
/// `export ... from "..."` (a re-export specifier) from local
/// `export function`/`export const` declarations whose bodies
/// may contain unrelated quoted strings.
fn contains_from_keyword(bytes: &[u8]) -> bool {
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' || c == b'\'' || c == b'`' {
            let q = c;
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            }
            continue;
        }
        if matches_keyword(bytes, i, b"from") {
            return true;
        }
        i += 1;
    }
    false
}

/// Find the byte index of the end of the current import/export
/// statement (`;` or newline at brace-depth zero, skipping
/// strings and comments).
fn find_statement_end(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 0i32;
    while i < bytes.len() {
        let c = bytes[i];
        // String literal: skip.
        if c == b'"' || c == b'\'' || c == b'`' {
            let q = c;
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            }
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth < 0 {
                return i;
            }
        } else if (c == b';' || c == b'\n') && depth == 0 {
            // Don't include the terminator itself.
            return i;
        }
        i += 1;
    }
    i
}

/// Return the contents of the last single- or double-quoted
/// string literal in `s` (excluding the surrounding quotes,
/// minimally unescaped). Returns None if no such literal exists.
fn find_last_string_literal(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut last: Option<(usize, usize, u8)> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            }
            continue;
        }
        if c == b'"' || c == b'\'' {
            let q = c;
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            last = Some((start, i, q));
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    last.map(|(start, end, _)| unescape_minimal(&s[start..end]))
}

fn unescape_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                match n {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\\' => out.push('\\'),
                    '\'' => out.push('\''),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Resolver: bare specifiers and relative paths
// ---------------------------------------------------------------------------

/// Resolution result for a single import specifier.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Same-compartment relative file. The path is canonicalised
    /// and is guaranteed to live inside the importing
    /// compartment's root directory.
    Relative {
        abs_path: PathBuf,
        /// The specifier as it appears in the synthesised
        /// compartment-map: the importing compartment-relative
        /// specifier (`./sibling.js`, `./lib/util.js`, with the
        /// extension preserved when present).
        compartment_specifier: String,
        parser: &'static str,
    },
    /// Bare specifier resolved via `node_modules` lookup.
    /// `package_root` is the directory containing the resolved
    /// `package.json`; `entry_file` is the absolute path of the
    /// package's chosen entry source; `subpath` is the
    /// package-rooted specifier (`"."` for the main entry,
    /// `"./sub.js"` for a subpath import).
    Bare {
        package_name: String,
        package_version: String,
        package_root: PathBuf,
        entry_file: PathBuf,
        compartment_specifier: String,
        parser: &'static str,
    },
}

/// Resolve `specifier` from the perspective of `importer_abs`.
///
/// - A specifier beginning with `./` or `../` is treated as
///   relative: resolved against `importer_abs`'s parent, walking
///   the usual extension fall-back (`.js`, `.mjs`, `.cjs`,
///   `.json`, then `index.<ext>` for directories).
/// - Any other specifier is treated as bare and resolved against
///   the nearest `node_modules` directory upward from
///   `importer_abs`, honouring the `name` segment (and one
///   `@scope/` prefix for scoped packages) and optional
///   `subpath` (the trailing `/...` part of the specifier).
pub fn resolve_specifier(
    importer_abs: &Path,
    specifier: &str,
    importer_compartment_root: &Path,
) -> io::Result<Resolved> {
    let importer_dir = importer_abs
        .parent()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("importer has no parent: {}", importer_abs.display()),
            )
        })?
        .to_path_buf();

    if specifier.starts_with("./") || specifier.starts_with("../") {
        let (abs_path, parser) = resolve_relative(&importer_dir, specifier)?;
        let canonical_root = importer_compartment_root
            .canonicalize()
            .unwrap_or_else(|_| importer_compartment_root.to_path_buf());
        if !abs_path.starts_with(&canonical_root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "relative import {specifier} escapes the importing compartment root {}",
                    canonical_root.display()
                ),
            ));
        }
        let rel = abs_path.strip_prefix(&canonical_root).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("strip prefix failed: {e}"),
            )
        })?;
        let compartment_specifier = format!("./{}", path_to_forward_slashes(rel));
        Ok(Resolved::Relative {
            abs_path,
            compartment_specifier,
            parser,
        })
    } else {
        resolve_bare(&importer_dir, specifier)
    }
}

fn resolve_relative(importer_dir: &Path, specifier: &str) -> io::Result<(PathBuf, &'static str)> {
    let base = importer_dir.join(specifier);
    // 1. Exact file (with author-written extension).
    if base.is_file() {
        let parser = parser_for_ext(base.extension()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported extension for {}: only .js/.mjs/.cjs/.json are supported",
                    base.display()
                ),
            )
        })?;
        let canonical = base.canonicalize()?;
        return Ok((canonical, parser));
    }
    // 2. Try extension fall-backs in priority order.
    for ext in ["js", "mjs", "cjs", "json"] {
        let candidate = base.with_extension(ext);
        if candidate.is_file() {
            let parser = parser_for_ext(Some(OsStr::new(ext))).unwrap();
            return Ok((candidate.canonicalize()?, parser));
        }
    }
    // 3. Directory with index.<ext>.
    if base.is_dir() {
        for ext in ["js", "mjs", "cjs", "json"] {
            let candidate = base.join(format!("index.{ext}"));
            if candidate.is_file() {
                let parser = parser_for_ext(Some(OsStr::new(ext))).unwrap();
                return Ok((candidate.canonicalize()?, parser));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "relative import {} not found from {}",
            specifier,
            importer_dir.display()
        ),
    ))
}

/// Split a bare specifier into `(package_name, subpath)`.
///
/// `"lodash"` → `("lodash", None)`.
/// `"lodash/fp"` → `("lodash", Some("fp"))`.
/// `"@scope/pkg"` → `("@scope/pkg", None)`.
/// `"@scope/pkg/sub/foo.js"` → `("@scope/pkg", Some("sub/foo.js"))`.
pub fn split_bare_specifier(specifier: &str) -> Option<(String, Option<String>)> {
    if specifier.is_empty() {
        return None;
    }
    if specifier.starts_with('@') {
        // Scoped: name is `@scope/pkg`; subpath is everything
        // after the second `/`.
        let mut parts = specifier.splitn(3, '/');
        let scope = parts.next()?;
        let pkg = parts.next()?;
        if scope.len() < 2 || pkg.is_empty() {
            return None;
        }
        let name = format!("{scope}/{pkg}");
        let subpath = parts.next().map(|s| s.to_string());
        Some((name, subpath))
    } else {
        let mut parts = specifier.splitn(2, '/');
        let pkg = parts.next()?;
        if pkg.is_empty() {
            return None;
        }
        let subpath = parts.next().map(|s| s.to_string());
        Some((pkg.to_string(), subpath))
    }
}

fn resolve_bare(importer_dir: &Path, specifier: &str) -> io::Result<Resolved> {
    let (pkg_name, subpath) = split_bare_specifier(specifier).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("malformed bare specifier: {specifier}"),
        )
    })?;

    let pkg_root = find_node_modules_package(importer_dir, &pkg_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "bare specifier {specifier} not found in node_modules upward from {}",
                importer_dir.display()
            ),
        )
    })?;

    let pkg_meta = load_package_metadata(&pkg_root)?;

    // Subpath resolution.
    let entry_file = match &subpath {
        None => resolve_package_main(&pkg_root, &pkg_meta)?,
        Some(sub) => {
            let candidate = pkg_root.join(sub);
            resolve_subpath(&pkg_root, &candidate, sub)?
        }
    };

    let parser = parser_for_ext(entry_file.extension()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "package {} resolved to {} with unsupported extension",
                pkg_name,
                entry_file.display()
            ),
        )
    })?;

    // Compartment specifier: `.` for the package main, `./sub`
    // for a subpath import. The subpath's extension is preserved
    // so `./lib/util.js` and `./lib/util` are distinct module
    // entries.
    let rel = entry_file.strip_prefix(&pkg_root).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("entry file outside package root: {e}"),
        )
    })?;
    let compartment_specifier = match &subpath {
        None => ".".to_string(),
        Some(_) => format!("./{}", path_to_forward_slashes(rel)),
    };

    Ok(Resolved::Bare {
        package_name: pkg_meta.name.clone(),
        package_version: pkg_meta.version.clone(),
        package_root: pkg_root,
        entry_file,
        compartment_specifier,
        parser,
    })
}

fn resolve_subpath(pkg_root: &Path, candidate: &Path, raw_sub: &str) -> io::Result<PathBuf> {
    // 1. Exact file (with author-written extension).
    if candidate.is_file() {
        let canonical = candidate.canonicalize()?;
        if !canonical.starts_with(pkg_root.canonicalize()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("subpath {raw_sub} escapes package root"),
            ));
        }
        return Ok(canonical);
    }
    for ext in ["js", "mjs", "cjs", "json"] {
        let with_ext = candidate.with_extension(ext);
        if with_ext.is_file() {
            return with_ext.canonicalize();
        }
    }
    if candidate.is_dir() {
        for ext in ["js", "mjs", "cjs", "json"] {
            let idx = candidate.join(format!("index.{ext}"));
            if idx.is_file() {
                return idx.canonicalize();
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("subpath {raw_sub} not found under {}", pkg_root.display()),
    ))
}

/// Walk upward from `start_dir`, returning the first directory
/// `<dir>/node_modules/<pkg_name>` that exists.
fn find_node_modules_package(start_dir: &Path, pkg_name: &str) -> Option<PathBuf> {
    let mut cursor: &Path = start_dir;
    loop {
        let candidate = cursor.join("node_modules").join(pkg_name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => return None,
        }
    }
}

/// Pick a package's entry source file from its `package.json`.
///
/// Resolution order (matching the design's "exports default →
/// main → index.js" cascade):
///
/// 1. `exports.["."]` when present. Honoured as either a string
///    (`"./index.mjs"`) or an object with a `default` key. Other
///    conditional keys (`browser`, `import`, `require`,
///    `node`, etc.) are not consulted; the registry-table walk
///    in a later phase covers conditional exports more fully.
/// 2. `module` field (some packages use this for the ESM entry).
/// 3. `main` field.
/// 4. `index.js` (with the usual extension fall-back) in the
///    package root.
fn resolve_package_main(pkg_root: &Path, pkg: &PackageMetadata) -> io::Result<PathBuf> {
    if let Some(rel) = pkg.exports_dot_default.as_deref() {
        let candidate = pkg_root.join(rel);
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    if let Some(rel) = pkg.module.as_deref() {
        let candidate = pkg_root.join(rel);
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    if let Some(rel) = pkg.main.as_deref() {
        let candidate = pkg_root.join(rel);
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    for ext in ["js", "mjs", "cjs", "json"] {
        let candidate = pkg_root.join(format!("index.{ext}"));
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "package {} at {} has no resolvable entry (exports.\".\".default/module/main/index.*)",
            pkg.name,
            pkg_root.display()
        ),
    ))
}

// ---------------------------------------------------------------------------
// package.json metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub main: Option<String>,
    pub module: Option<String>,
    pub exports_dot_default: Option<String>,
}

/// Read and parse `package.json` from `pkg_root`. Missing
/// optional fields are absent in the returned struct; `name` and
/// `version` fall back to placeholders (the directory basename
/// and `"0.0.0"`) when the JSON does not declare them.
pub fn load_package_metadata(pkg_root: &Path) -> io::Result<PackageMetadata> {
    let path = pkg_root.join("package.json");
    let bytes = std::fs::read(&path)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid package.json at {}: {e}", path.display()),
        )
    })?;

    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            pkg_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "anonymous".to_string())
        });
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "0.0.0".to_string());
    let main = v
        .get("main")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let module = v
        .get("module")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());

    // `exports`: support the two shapes the design names.
    // - `"exports": "./index.mjs"` (shorthand)
    // - `"exports": { ".": "./index.mjs" }` (string subpath)
    // - `"exports": { ".": { "default": "./index.mjs" } }`
    //   (conditional)
    let exports_dot_default = v.get("exports").and_then(|exp| match exp {
        serde_json::Value::String(s) => Some(s.to_string()),
        serde_json::Value::Object(map) => match map.get(".") {
            Some(serde_json::Value::String(s)) => Some(s.to_string()),
            Some(serde_json::Value::Object(cond)) => cond
                .get("default")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            _ => None,
        },
        _ => None,
    });

    Ok(PackageMetadata {
        name,
        version,
        main,
        module,
        exports_dot_default,
    })
}

fn compartment_id_for(name: &str, version: &str) -> String {
    // The `@endo/compartment-mapper` output uses
    // `<unscoped-name>-v<version>` for scoped packages and
    // `<name>-v<version>` for unscoped. We mirror that
    // convention so the synthesised archives are recognisable
    // alongside ZIP-shaped ones.
    let unscoped = name.rsplit('/').next().unwrap_or(name);
    format!("{unscoped}-v{version}")
}

fn path_to_forward_slashes(p: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for comp in p.components() {
        if let std::path::Component::Normal(c) = comp {
            parts.push(c.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
}

// ---------------------------------------------------------------------------
// Walker: graph traversal + CAS emission
// ---------------------------------------------------------------------------

struct Compartment {
    /// `<unscoped-name>-v<version>` (or `entry-v1.0.0` for the
    /// entry compartment when the entry has no `package.json`).
    id: String,
    /// Human-readable name (the `name` field from `package.json`,
    /// or `"entry"`). Surfaces as the compartment's `name`
    /// property in the synthesised `compartment-map.json`.
    name: String,
    /// Filesystem root the compartment is anchored at. All file
    /// paths recorded in `modules` below are descendants.
    root: PathBuf,
    /// Per-specifier module record. Keys are compartment-rooted
    /// specifiers (`"./index.js"`, `"./lib/util.js"`, `"."` for
    /// a package's main entry). Values are either a `File`
    /// (in-tree source) or a `Link` (cross-compartment).
    modules: HashMap<String, ModuleRecord>,
    /// `(compartment_id, specifier)` for in-tree sources
    /// recorded in the order they were observed; used to keep
    /// the CAS tree's per-package file ordering deterministic
    /// when serialising.
    in_order_specs: Vec<String>,
}

enum ModuleRecord {
    File {
        /// Path on disk of the source file. The walker writes
        /// the file into the CAS exactly once even if the same
        /// file is reached via multiple specifiers in the same
        /// compartment.
        abs_path: PathBuf,
        /// `mjs`, `cjs`, or `json`.
        parser: &'static str,
        /// The compartment-rooted file location written to the
        /// compartment-map.json's `location` field. Stable
        /// across runs because it is derived from the path
        /// relative to `Compartment::root`.
        location: String,
    },
    Link {
        target_compartment_id: String,
        target_specifier: String,
    },
}

struct WalkerQueueItem {
    compartment_id: String,
    specifier: String,
    abs_path: PathBuf,
    parser: String,
}

struct Walker<'a> {
    cas: &'a ContentStore,
    compartments: HashMap<String, Compartment>,
    /// Map from package_root canonical path to compartment id,
    /// so two bare imports of the same package via different
    /// importers share one compartment.
    pkg_root_to_id: HashMap<PathBuf, String>,
    queue: Vec<WalkerQueueItem>,
    /// Per-compartment set of file paths already enqueued, to
    /// stop the BFS from re-visiting the same file twice.
    enqueued_in_compartment: HashMap<String, std::collections::HashSet<PathBuf>>,
}

impl<'a> Walker<'a> {
    fn new(cas: &'a ContentStore) -> Self {
        Self {
            cas,
            compartments: HashMap::new(),
            pkg_root_to_id: HashMap::new(),
            queue: Vec::new(),
            enqueued_in_compartment: HashMap::new(),
        }
    }

    fn add_compartment(&mut self, id: String, name: String, root: PathBuf) {
        let canonical_root = root.canonicalize().unwrap_or(root);
        self.pkg_root_to_id
            .insert(canonical_root.clone(), id.clone());
        self.compartments.entry(id.clone()).or_insert(Compartment {
            id,
            name,
            root: canonical_root,
            modules: HashMap::new(),
            in_order_specs: Vec::new(),
        });
    }

    fn enqueue(
        &mut self,
        compartment_id: String,
        specifier: String,
        abs_path: PathBuf,
        parser: String,
    ) {
        let set = self
            .enqueued_in_compartment
            .entry(compartment_id.clone())
            .or_default();
        if !set.insert(abs_path.clone()) {
            // Already enqueued / visited in this compartment.
            // Still ensure the specifier-to-file mapping is
            // recorded so the compartment-map carries it.
            return;
        }
        self.queue.push(WalkerQueueItem {
            compartment_id,
            specifier,
            abs_path,
            parser,
        });
    }

    fn drain(&mut self) -> io::Result<()> {
        while let Some(item) = self.queue.pop() {
            self.visit(item)?;
        }
        Ok(())
    }

    fn visit(&mut self, item: WalkerQueueItem) -> io::Result<()> {
        let WalkerQueueItem {
            compartment_id,
            specifier,
            abs_path,
            parser,
        } = item;

        // Read the source so we can scan its imports.
        let source = std::fs::read_to_string(&abs_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot read {} as utf-8: {e}", abs_path.display()),
            )
        })?;

        // Record the module entry in its compartment.
        let comp_root = {
            let comp = self.compartments.get(&compartment_id).ok_or_else(|| {
                io::Error::other(format!("walker missing compartment {compartment_id}"))
            })?;
            comp.root.clone()
        };

        let rel = abs_path.strip_prefix(&comp_root).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} not under compartment root {}: {e}",
                    abs_path.display(),
                    comp_root.display()
                ),
            )
        })?;
        let location = path_to_forward_slashes(rel);

        let static_parser: &'static str = match parser.as_str() {
            "mjs" => "mjs",
            "cjs" => "cjs",
            "json" => "json",
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported parser {other} for {}", abs_path.display()),
                ));
            }
        };

        {
            let comp = self.compartments.get_mut(&compartment_id).unwrap();
            comp.modules.insert(
                specifier.clone(),
                ModuleRecord::File {
                    abs_path: abs_path.clone(),
                    parser: static_parser,
                    location,
                },
            );
            if !comp.in_order_specs.contains(&specifier) {
                comp.in_order_specs.push(specifier.clone());
            }
        }

        // JSON files don't contain imports.
        if static_parser == "json" {
            return Ok(());
        }

        // Scan imports and walk each.
        let scan = scan_static_imports(&source);
        for spec in &scan.specifiers {
            self.handle_import(&compartment_id, &abs_path, spec)?;
        }

        Ok(())
    }

    fn handle_import(
        &mut self,
        compartment_id: &str,
        importer_abs: &Path,
        spec: &str,
    ) -> io::Result<()> {
        let comp_root = {
            let comp = self.compartments.get(compartment_id).ok_or_else(|| {
                io::Error::other(format!("missing compartment {compartment_id}"))
            })?;
            comp.root.clone()
        };

        let resolved = resolve_specifier(importer_abs, spec, &comp_root)?;
        match resolved {
            Resolved::Relative {
                abs_path,
                compartment_specifier,
                parser,
            } => {
                // Record a File entry pointing at the resolved
                // file under the importing compartment's
                // specifier. (The visit will re-derive the
                // identical record but that is fine; the second
                // write is idempotent.)
                {
                    let comp = self.compartments.get_mut(compartment_id).unwrap();
                    let rel = abs_path.strip_prefix(&comp.root).map_err(|e| {
                        io::Error::new(io::ErrorKind::InvalidInput, format!("strip: {e}"))
                    })?;
                    let location = path_to_forward_slashes(rel);
                    comp.modules.entry(compartment_specifier.clone()).or_insert(
                        ModuleRecord::File {
                            abs_path: abs_path.clone(),
                            parser,
                            location,
                        },
                    );
                    if !comp.in_order_specs.contains(&compartment_specifier) {
                        comp.in_order_specs.push(compartment_specifier.clone());
                    }
                }
                self.enqueue(
                    compartment_id.to_string(),
                    compartment_specifier,
                    abs_path,
                    parser.to_string(),
                );
            }
            Resolved::Bare {
                package_name,
                package_version,
                package_root,
                entry_file,
                compartment_specifier,
                parser,
            } => {
                let target_compartment_id = match self.pkg_root_to_id.get(&package_root) {
                    Some(id) => id.clone(),
                    None => {
                        let id = compartment_id_for(&package_name, &package_version);
                        self.add_compartment(
                            id.clone(),
                            package_name.clone(),
                            package_root.clone(),
                        );
                        id
                    }
                };

                // The importing compartment gets a Link entry
                // under the *bare specifier the source author
                // wrote* so that XS's resolve hook returns
                // exactly the specifier string the import
                // statement contains.
                {
                    let comp = self.compartments.get_mut(compartment_id).unwrap();
                    comp.modules.insert(
                        spec.to_string(),
                        ModuleRecord::Link {
                            target_compartment_id: target_compartment_id.clone(),
                            target_specifier: compartment_specifier.clone(),
                        },
                    );
                    if !comp.in_order_specs.contains(&spec.to_string()) {
                        comp.in_order_specs.push(spec.to_string());
                    }
                }

                // Enqueue the target compartment's entry file.
                self.enqueue(
                    target_compartment_id,
                    compartment_specifier,
                    entry_file,
                    parser.to_string(),
                );
            }
        }
        Ok(())
    }

    /// Serialise the gathered compartments to a
    /// compartment-map.json text.
    fn emit_map_json(&self, entry_compartment_id: &str, entry_specifier: &str) -> String {
        // Sort compartment ids and the per-compartment module
        // specifiers so the output JSON is byte-deterministic
        // for a given graph; this matches Phase 3's
        // determinism contract and is required for stable CAS
        // root hashes across runs.
        let mut comp_ids: Vec<&String> = self.compartments.keys().collect();
        comp_ids.sort();

        let mut buf = String::new();
        buf.push('{');

        // entry
        buf.push_str(&format!(
            r#""entry":{{"compartment":{},"module":{}}}"#,
            json_escape(entry_compartment_id),
            json_escape(entry_specifier),
        ));
        buf.push(',');

        // compartments
        buf.push_str(r#""compartments":{"#);
        for (i, cid) in comp_ids.iter().enumerate() {
            if i > 0 {
                buf.push(',');
            }
            let comp = &self.compartments[*cid];
            buf.push_str(&format!(
                r#"{}:{{"name":{},"modules":{{"#,
                json_escape(&comp.id),
                json_escape(&comp.name),
            ));

            let mut specs: Vec<&String> = comp.modules.keys().collect();
            specs.sort();
            for (j, spec) in specs.iter().enumerate() {
                if j > 0 {
                    buf.push(',');
                }
                let rec = &comp.modules[*spec];
                buf.push_str(&format!("{}:", json_escape(spec)));
                match rec {
                    ModuleRecord::File {
                        parser, location, ..
                    } => {
                        buf.push_str(&format!(
                            r#"{{"parser":{},"location":{}}}"#,
                            json_escape(parser),
                            json_escape(location),
                        ));
                    }
                    ModuleRecord::Link {
                        target_compartment_id,
                        target_specifier,
                    } => {
                        buf.push_str(&format!(
                            r#"{{"compartment":{},"module":{}}}"#,
                            json_escape(target_compartment_id),
                            json_escape(target_specifier),
                        ));
                    }
                }
            }
            buf.push_str("}}");
        }
        buf.push('}');

        buf.push('}');
        buf
    }

    /// Write the per-compartment file blobs, the compartment
    /// subtrees, the compartment-map.json blob, and the root
    /// tree to the CAS. Returns the root hash.
    ///
    /// Determinism contract: the tree manifests written to the
    /// CAS use [`serialize_tree_sorted`] (sorted by key) so the
    /// same dependency graph produces a byte-identical root hash
    /// across runs. `TreeManifest`'s default serialisation goes
    /// through a `HashMap`, whose iteration order is randomised
    /// per-process; relying on the default would give the
    /// walker a different root hash on every invocation. The
    /// per-compartment module-spec sort is for the same reason.
    fn write_root_tree(&self, map_json: &str) -> io::Result<String> {
        let map_hash = self.cas.store(map_json.as_bytes(), "blob")?;
        let map_size = map_json.len() as u64;

        let mut root_entries: Vec<(String, TreeEntry)> = Vec::new();
        root_entries.push((
            "compartment-map.json".to_string(),
            TreeEntry {
                entry_type: "blob".to_string(),
                hash: map_hash,
                size: Some(map_size),
            },
        ));

        // Sort compartment ids for determinism.
        let mut comp_ids: Vec<&String> = self.compartments.keys().collect();
        comp_ids.sort();
        for cid in comp_ids {
            let comp = &self.compartments[cid];
            let mut sub_entries: Vec<(String, TreeEntry)> = Vec::new();
            // Sort module specifiers for determinism.
            let mut specs: Vec<&String> = comp.modules.keys().collect();
            specs.sort();
            // Track files already written under this compartment
            // to avoid duplicate writes when two specifiers
            // point at the same disk file (e.g., a relative
            // import that also serves as a package main).
            let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
            for spec in specs {
                if let ModuleRecord::File {
                    abs_path, location, ..
                } = &comp.modules[spec]
                {
                    if !written.insert(location.clone()) {
                        continue;
                    }
                    let bytes = std::fs::read(abs_path)?;
                    let hash = self.cas.store(&bytes, "blob")?;
                    sub_entries.push((
                        location.clone(),
                        TreeEntry {
                            entry_type: "blob".to_string(),
                            hash,
                            size: Some(bytes.len() as u64),
                        },
                    ));
                }
            }
            let sub_json = serialize_tree_sorted(&sub_entries);
            let sub_hash = self.cas.store_tree(sub_json.as_bytes())?;
            root_entries.push((
                cid.clone(),
                TreeEntry {
                    entry_type: "tree".to_string(),
                    hash: sub_hash,
                    size: None,
                },
            ));
        }

        let root_json = serialize_tree_sorted(&root_entries);
        self.cas.store_tree(root_json.as_bytes())
    }
}

/// Serialise a tree manifest's entries as JSON with keys in
/// ascending byte order. The shape is the same `TreeManifest`
/// shape `serde` would produce, but with deterministic key
/// ordering so the CAS hash of the resulting bytes depends only
/// on the (key, entry) set, not on `HashMap` iteration order.
///
/// Takes a `Vec<(String, TreeEntry)>` rather than a HashMap so
/// the caller can pass a pre-sorted vector when it already has
/// one, but the function still sorts defensively to make the
/// contract explicit.
fn serialize_tree_sorted(entries: &[(String, TreeEntry)]) -> String {
    let mut sorted: Vec<&(String, TreeEntry)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = String::from(r#"{"entries":{"#);
    for (i, (k, v)) in sorted.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        buf.push_str(&serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string()));
        buf.push(':');
        buf.push_str(&serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()));
    }
    buf.push_str("}}");
    buf
}

fn json_escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, contents: &[u8]) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents).unwrap();
    }

    // ---- scan_static_imports ----

    #[test]
    fn scan_extracts_default_named_and_namespace_imports() {
        let src = r#"
            import foo from "a";
            import { x, y as z } from 'b';
            import * as ns from "c";
            import "d";
            import baz, { q } from "e";
        "#;
        let s = scan_static_imports(src);
        assert_eq!(s.specifiers, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn scan_extracts_export_from() {
        let src = r#"
            export { a } from "m1";
            export * from 'm2';
            export * as k from "m3";
        "#;
        let s = scan_static_imports(src);
        assert_eq!(s.specifiers, vec!["m1", "m2", "m3"]);
    }

    #[test]
    fn scan_ignores_dynamic_import_and_meta() {
        let src = r#"
            const m = await import("x");
            const u = import.meta.url;
            import foo from "real";
        "#;
        let s = scan_static_imports(src);
        assert_eq!(s.specifiers, vec!["real"]);
    }

    #[test]
    fn scan_deduplicates() {
        let src = r#"
            import { a } from "dup";
            import { b } from "dup";
        "#;
        let s = scan_static_imports(src);
        assert_eq!(s.specifiers, vec!["dup"]);
    }

    #[test]
    fn scan_returns_empty_for_no_imports() {
        let src = "export const x = 42;\nconsole.log('hi');";
        let s = scan_static_imports(src);
        assert!(s.specifiers.is_empty());
    }

    // ---- split_bare_specifier ----

    #[test]
    fn split_unscoped_and_scoped() {
        assert_eq!(
            split_bare_specifier("lodash"),
            Some(("lodash".to_string(), None))
        );
        assert_eq!(
            split_bare_specifier("lodash/fp"),
            Some(("lodash".to_string(), Some("fp".to_string())))
        );
        assert_eq!(
            split_bare_specifier("@scope/pkg"),
            Some(("@scope/pkg".to_string(), None))
        );
        assert_eq!(
            split_bare_specifier("@scope/pkg/sub/a.js"),
            Some(("@scope/pkg".to_string(), Some("sub/a.js".to_string())))
        );
    }

    #[test]
    fn split_rejects_empty() {
        assert_eq!(split_bare_specifier(""), None);
        assert_eq!(split_bare_specifier("@"), None);
    }

    // ---- load_package_metadata ----

    #[test]
    fn load_package_metadata_reads_name_version_main() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "package.json",
            br#"{"name":"foo","version":"1.2.3","main":"./lib/entry.js"}"#,
        );
        let m = load_package_metadata(dir.path()).unwrap();
        assert_eq!(m.name, "foo");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.main.as_deref(), Some("./lib/entry.js"));
        assert!(m.exports_dot_default.is_none());
    }

    #[test]
    fn load_package_metadata_reads_exports_default() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "package.json",
            br#"{"name":"foo","version":"2.0.0","exports":{".":{"default":"./esm.js"}}}"#,
        );
        let m = load_package_metadata(dir.path()).unwrap();
        assert_eq!(m.exports_dot_default.as_deref(), Some("./esm.js"));
    }

    #[test]
    fn load_package_metadata_reads_exports_dot_string() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "package.json",
            br#"{"name":"foo","version":"2.0.0","exports":{".":"./shorthand.js"}}"#,
        );
        let m = load_package_metadata(dir.path()).unwrap();
        assert_eq!(m.exports_dot_default.as_deref(), Some("./shorthand.js"));
    }

    #[test]
    fn load_package_metadata_fallback_for_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "package.json", b"{}");
        let m = load_package_metadata(dir.path()).unwrap();
        // name falls back to the directory basename.
        let dirname = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(m.name, dirname);
        assert_eq!(m.version, "0.0.0");
        assert!(m.main.is_none());
        assert!(m.exports_dot_default.is_none());
    }

    #[test]
    fn load_package_metadata_rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "package.json", b"{ not valid");
        let err = load_package_metadata(dir.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("invalid package.json"));
    }

    // ---- find_node_modules_package ----

    #[test]
    fn find_walks_upward_to_node_modules() {
        let root = tempfile::tempdir().unwrap();
        write_file(
            root.path(),
            "node_modules/foo/package.json",
            br#"{"name":"foo","version":"1.0.0"}"#,
        );
        std::fs::create_dir_all(root.path().join("a/b/c")).unwrap();
        let found = find_node_modules_package(&root.path().join("a/b/c"), "foo").unwrap();
        assert_eq!(found, root.path().join("node_modules/foo"));
    }

    #[test]
    fn find_handles_scoped_package() {
        let root = tempfile::tempdir().unwrap();
        write_file(
            root.path(),
            "node_modules/@scope/pkg/package.json",
            br#"{"name":"@scope/pkg","version":"0.1.0"}"#,
        );
        let found = find_node_modules_package(root.path(), "@scope/pkg").unwrap();
        assert_eq!(found, root.path().join("node_modules/@scope/pkg"));
    }

    #[test]
    fn find_returns_none_when_absent() {
        let root = tempfile::tempdir().unwrap();
        assert!(find_node_modules_package(root.path(), "missing").is_none());
    }

    // ---- resolve_specifier ----

    #[test]
    fn resolve_relative_with_explicit_extension() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(root.path(), "src/util.js", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "./util.js", root.path()).unwrap();
        match r {
            Resolved::Relative {
                abs_path,
                compartment_specifier,
                parser,
            } => {
                assert_eq!(
                    abs_path,
                    root.path().canonicalize().unwrap().join("src/util.js")
                );
                assert_eq!(compartment_specifier, "./src/util.js");
                assert_eq!(parser, "mjs");
            }
            _ => panic!("expected Relative"),
        }
    }

    #[test]
    fn resolve_relative_with_extension_fallback() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(root.path(), "src/util.mjs", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "./util", root.path()).unwrap();
        if let Resolved::Relative {
            abs_path, parser, ..
        } = r
        {
            assert_eq!(abs_path.extension().unwrap(), "mjs");
            assert_eq!(parser, "mjs");
        } else {
            panic!("expected Relative");
        }
    }

    #[test]
    fn resolve_relative_directory_index() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(root.path(), "src/sub/index.js", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "./sub", root.path()).unwrap();
        if let Resolved::Relative { abs_path, .. } = r {
            assert_eq!(
                abs_path,
                root.path().canonicalize().unwrap().join("src/sub/index.js")
            );
        } else {
            panic!("expected Relative");
        }
    }

    #[test]
    fn resolve_relative_rejects_escape() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "outer/sibling.js", b"x");
        write_file(root.path(), "inner/main.js", b"x");
        let importer = root.path().join("inner/main.js");
        let err = resolve_specifier(&importer, "../outer/sibling.js", &root.path().join("inner"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("escapes"));
    }

    #[test]
    fn resolve_bare_finds_package_main() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(
            root.path(),
            "node_modules/lib/package.json",
            br#"{"name":"lib","version":"0.5.0","main":"./entry.js"}"#,
        );
        write_file(root.path(), "node_modules/lib/entry.js", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "lib", &root.path().join("src")).unwrap();
        if let Resolved::Bare {
            package_name,
            package_version,
            entry_file,
            compartment_specifier,
            parser,
            ..
        } = r
        {
            assert_eq!(package_name, "lib");
            assert_eq!(package_version, "0.5.0");
            assert_eq!(compartment_specifier, ".");
            assert_eq!(parser, "mjs");
            assert!(entry_file.ends_with("entry.js"));
        } else {
            panic!("expected Bare");
        }
    }

    #[test]
    fn resolve_bare_falls_back_to_index_js() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(
            root.path(),
            "node_modules/lib/package.json",
            br#"{"name":"lib","version":"0.0.1"}"#,
        );
        write_file(root.path(), "node_modules/lib/index.js", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "lib", &root.path().join("src")).unwrap();
        if let Resolved::Bare { entry_file, .. } = r {
            assert!(entry_file.ends_with("index.js"));
        } else {
            panic!("expected Bare");
        }
    }

    #[test]
    fn resolve_bare_subpath() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        write_file(
            root.path(),
            "node_modules/lib/package.json",
            br#"{"name":"lib","version":"0.0.1"}"#,
        );
        write_file(root.path(), "node_modules/lib/sub/foo.js", b"x");
        let importer = root.path().join("src/main.js");
        let r = resolve_specifier(&importer, "lib/sub/foo.js", &root.path().join("src")).unwrap();
        if let Resolved::Bare {
            entry_file,
            compartment_specifier,
            ..
        } = r
        {
            assert!(entry_file.ends_with("sub/foo.js"));
            assert_eq!(compartment_specifier, "./sub/foo.js");
        } else {
            panic!("expected Bare");
        }
    }

    #[test]
    fn resolve_bare_missing_yields_not_found() {
        let root = tempfile::tempdir().unwrap();
        write_file(root.path(), "src/main.js", b"x");
        let importer = root.path().join("src/main.js");
        let err = resolve_specifier(&importer, "absent", &root.path().join("src")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("absent"));
    }

    // ---- ingest_entry_point_with_deps end-to-end ----

    #[test]
    fn ingest_walks_relative_imports_into_one_compartment() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();

        let proj = tempfile::tempdir().unwrap();
        write_file(
            proj.path(),
            "main.js",
            br#"import { greet } from './util.js'; greet();"#,
        );
        write_file(
            proj.path(),
            "util.js",
            br#"export function greet() { return 'hi'; }"#,
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("main.js")).unwrap();

        // One compartment (entry-v1.0.0), two modules in it.
        assert_eq!(ingested.archive.map.compartments.len(), 1);
        let comp = ingested
            .archive
            .map
            .compartments
            .get(SYNTHETIC_COMPARTMENT_ID)
            .unwrap();
        assert_eq!(comp.modules.len(), 2);
        assert!(comp.modules.contains_key("./main.js"));
        assert!(comp.modules.contains_key("./util.js"));

        // Both source bodies are present.
        let main_src = ingested
            .archive
            .sources
            .get(&(
                SYNTHETIC_COMPARTMENT_ID.to_string(),
                "./main.js".to_string(),
            ))
            .unwrap();
        assert!(main_src.contains("greet"));
        let util_src = ingested
            .archive
            .sources
            .get(&(
                SYNTHETIC_COMPARTMENT_ID.to_string(),
                "./util.js".to_string(),
            ))
            .unwrap();
        assert!(util_src.contains("return 'hi'"));
    }

    #[test]
    fn ingest_walks_bare_import_into_separate_compartment() {
        // The Phase 5 acceptance test: `endor run app.js` where
        // `app.js` imports from a local `node_modules` package.
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();

        let proj = tempfile::tempdir().unwrap();
        write_file(
            proj.path(),
            "app.js",
            br#"import { add } from 'mathlib'; add(1, 2);"#,
        );
        write_file(
            proj.path(),
            "node_modules/mathlib/package.json",
            br#"{"name":"mathlib","version":"3.4.5","main":"./entry.js"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/mathlib/entry.js",
            br#"export function add(a, b) { return a + b; }"#,
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("app.js")).unwrap();

        // Two compartments: the entry compartment and the
        // mathlib compartment.
        assert_eq!(ingested.archive.map.compartments.len(), 2);
        let entry_comp = ingested
            .archive
            .map
            .compartments
            .get(SYNTHETIC_COMPARTMENT_ID)
            .unwrap();
        let mathlib_id = "mathlib-v3.4.5";
        let mathlib_comp = ingested.archive.map.compartments.get(mathlib_id).unwrap();

        // The entry compartment has a Link entry under the bare
        // specifier `mathlib`.
        match entry_comp.modules.get("mathlib").unwrap() {
            xsnap::archive::ModuleDescriptor::Link {
                compartment,
                module,
            } => {
                assert_eq!(compartment, mathlib_id);
                assert_eq!(module, ".");
            }
            other => panic!("expected Link, got {other:?}"),
        }

        // The mathlib compartment has the package's entry source
        // under specifier `.`.
        assert!(mathlib_comp.modules.contains_key("."));
        let src = ingested
            .archive
            .sources
            .get(&(mathlib_id.to_string(), ".".to_string()))
            .unwrap();
        assert!(src.contains("function add"));
    }

    #[test]
    fn ingest_walks_scoped_bare_import() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();

        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.mjs", br#"import x from '@scope/pkg';"#);
        write_file(
            proj.path(),
            "node_modules/@scope/pkg/package.json",
            br#"{"name":"@scope/pkg","version":"1.0.0","exports":{".":"./esm.mjs"}}"#,
        );
        write_file(
            proj.path(),
            "node_modules/@scope/pkg/esm.mjs",
            br#"export default 1;"#,
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("app.mjs")).unwrap();

        // The scoped compartment id is `pkg-v1.0.0` (the
        // unscoped name) per the compartment_id_for rule.
        let scoped_id = "pkg-v1.0.0";
        assert!(ingested.archive.map.compartments.contains_key(scoped_id));
        let comp = ingested.archive.map.compartments.get(scoped_id).unwrap();
        assert!(comp.modules.contains_key("."));
    }

    #[test]
    fn ingest_walks_transitive_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();

        // app -> a -> b
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.js", br#"import { f } from 'a'; f();"#);
        write_file(
            proj.path(),
            "node_modules/a/package.json",
            br#"{"name":"a","version":"1.0.0"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/a/index.js",
            br#"import { g } from 'b'; export function f() { return g(); }"#,
        );
        write_file(
            proj.path(),
            "node_modules/b/package.json",
            br#"{"name":"b","version":"2.0.0"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/b/index.js",
            br#"export function g() { return 42; }"#,
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("app.js")).unwrap();

        // Three compartments: entry, a, b.
        assert_eq!(ingested.archive.map.compartments.len(), 3);
        assert!(ingested.archive.map.compartments.contains_key("a-v1.0.0"));
        assert!(ingested.archive.map.compartments.contains_key("b-v2.0.0"));

        // a's compartment has a Link to b.
        let a_comp = ingested.archive.map.compartments.get("a-v1.0.0").unwrap();
        match a_comp.modules.get("b").unwrap() {
            xsnap::archive::ModuleDescriptor::Link {
                compartment,
                module,
            } => {
                assert_eq!(compartment, "b-v2.0.0");
                assert_eq!(module, ".");
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn ingest_handles_relative_subdir_within_compartment() {
        // Importing `./sub/util.js` from `main.js` within the
        // entry compartment: both files become module entries
        // in the same compartment, sharing the synthetic id.
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        write_file(
            proj.path(),
            "main.js",
            br#"import { u } from './sub/util.js'; u();"#,
        );
        write_file(
            proj.path(),
            "sub/util.js",
            br#"export function u() { return 1; }"#,
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("main.js")).unwrap();
        let comp = ingested
            .archive
            .map
            .compartments
            .get(SYNTHETIC_COMPARTMENT_ID)
            .unwrap();
        assert!(comp.modules.contains_key("./main.js"));
        assert!(comp.modules.contains_key("./sub/util.js"));
    }

    #[test]
    fn ingest_uses_package_id_when_entry_has_package_json() {
        // When the entry directory has a `package.json`, the
        // entry compartment id is derived from the package
        // metadata rather than using the synthetic placeholder.
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();

        let proj = tempfile::tempdir().unwrap();
        write_file(
            proj.path(),
            "package.json",
            br#"{"name":"my-app","version":"0.7.0"}"#,
        );
        write_file(proj.path(), "main.js", b"export default 1;");

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("main.js")).unwrap();
        assert!(ingested
            .archive
            .map
            .compartments
            .contains_key("my-app-v0.7.0"));
        assert_eq!(ingested.archive.map.entry.compartment, "my-app-v0.7.0");
    }

    #[test]
    fn ingest_dedupes_shared_dependency() {
        // app -> a -> shared
        // app -> shared
        // The `shared` compartment must appear exactly once
        // and be reachable from both app and a via Link
        // entries pointing at the same compartment id.
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.js", br#"import 'a'; import 'shared';"#);
        write_file(
            proj.path(),
            "node_modules/a/package.json",
            br#"{"name":"a","version":"1.0.0"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/a/index.js",
            br#"import 'shared';"#,
        );
        write_file(
            proj.path(),
            "node_modules/shared/package.json",
            br#"{"name":"shared","version":"1.0.0"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/shared/index.js",
            b"export const x = 1;",
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("app.js")).unwrap();
        // entry, a, shared (no duplicates)
        assert_eq!(ingested.archive.map.compartments.len(), 3);
        assert!(ingested
            .archive
            .map
            .compartments
            .contains_key("shared-v1.0.0"));
    }

    #[test]
    fn ingest_surfaces_missing_bare_specifier() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.js", br#"import 'totally-missing';"#);

        let err = match ingest_entry_point_with_deps(&cas, &proj.path().join("app.js")) {
            Ok(_) => panic!("expected NotFound for missing bare specifier"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("totally-missing"));
    }

    #[test]
    fn ingest_reads_back_from_cas_root_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.js", br#"import { v } from 'lib';"#);
        write_file(
            proj.path(),
            "node_modules/lib/package.json",
            br#"{"name":"lib","version":"1.0.0"}"#,
        );
        write_file(
            proj.path(),
            "node_modules/lib/index.js",
            b"export const v = 1;",
        );

        let ingested = ingest_entry_point_with_deps(&cas, &proj.path().join("app.js")).unwrap();
        let root_hash = ingested.root_hash.clone();
        let reloaded = load_archive_from_cas(&cas, &root_hash).unwrap();
        assert_eq!(reloaded.map.compartments.len(), 2);
        assert!(reloaded.map.compartments.contains_key("lib-v1.0.0"));
    }

    #[test]
    fn ingest_is_deterministic_across_runs() {
        // The CAS root hash for the same graph is byte-stable
        // across runs because `emit_map_json` sorts compartment
        // ids and module specifiers and `write_root_tree` walks
        // both in sorted order.
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "app.js", br#"import 'a'; import 'b';"#);
        for pkg in ["a", "b"] {
            write_file(
                proj.path(),
                &format!("node_modules/{pkg}/package.json"),
                format!(r#"{{"name":"{pkg}","version":"1.0.0"}}"#).as_bytes(),
            );
            write_file(
                proj.path(),
                &format!("node_modules/{pkg}/index.js"),
                b"export const x = 1;",
            );
        }

        let tmp1 = tempfile::tempdir().unwrap();
        let cas1 = ContentStore::open(&tmp1.path().join("cas")).unwrap();
        let i1 = ingest_entry_point_with_deps(&cas1, &proj.path().join("app.js")).unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let cas2 = ContentStore::open(&tmp2.path().join("cas")).unwrap();
        let i2 = ingest_entry_point_with_deps(&cas2, &proj.path().join("app.js")).unwrap();
        assert_eq!(i1.root_hash, i2.root_hash);
    }

    #[test]
    fn ingest_rejects_unsupported_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        write_file(proj.path(), "main.txt", b"not js");
        let err = match ingest_entry_point_with_deps(&cas, &proj.path().join("main.txt")) {
            Ok(_) => panic!("expected error for unsupported extension"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err
            .to_string()
            .contains("unsupported entry-point extension"));
    }

    #[test]
    fn ingest_rejects_missing_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = ContentStore::open(&tmp.path().join("cas")).unwrap();
        let proj = tempfile::tempdir().unwrap();
        let err = match ingest_entry_point_with_deps(&cas, &proj.path().join("nope.js")) {
            Ok(_) => panic!("expected NotFound for missing entry"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
