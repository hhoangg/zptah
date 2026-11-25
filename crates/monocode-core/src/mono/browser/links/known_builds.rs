//! Which extension builds this app accepts (spec 13.5, decision 7: an unknown build refuses to
//! connect). The list is compiled in. When the installed extension folder can be read its file
//! hash is compared; otherwise the version the extension reports is compared, which stops an
//! accidental mismatch but not a malicious build (spec 13.5, threat T7).
//!
//! Native Dev trusts only the dev extension, which is rebuilt constantly, so it skips the list.
use super::super::peer::Flavor;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One released build. `sha256` is the package hash recorded in `extension-release.json`
/// (see [`hash_extension_dir`]); `None` until the release tooling fills it in, in which case the
/// version alone decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnownBuild {
    pub version: &'static str,
    pub sha256: Option<&'static str>,
}

/// Builds shipped with this app. An extension release is published only after the app release
/// that lists it (spec 13.6).
pub(crate) const KNOWN_BUILDS: &[KnownBuild] = &[KnownBuild {
    version: "1.0.0",
    sha256: None,
}];

/// Shown to the user, verbatim from the spec.
pub(crate) const UNKNOWN_BUILD_MESSAGE: &str =
    "This extension version is not known to this zptah. Update zptah.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuildVerdict {
    Accepted,
    /// The reported version is not in the list.
    UnknownVersion(String),
    /// The installed files hash to something the list does not hold for that version.
    HashMismatch(String),
}

#[derive(Debug, Clone)]
pub(crate) struct KnownBuilds {
    builds: Vec<KnownBuild>,
    flavor: Flavor,
}

impl KnownBuilds {
    pub(crate) fn compiled(flavor: Flavor) -> Self {
        Self::with(KNOWN_BUILDS.to_vec(), flavor)
    }

    pub(crate) fn with(builds: Vec<KnownBuild>, flavor: Flavor) -> Self {
        Self { builds, flavor }
    }

    /// `installed_dir` is the extension folder inside the profile when the app could read it.
    pub(crate) fn check(&self, ext_version: &str, installed_dir: Option<&Path>) -> BuildVerdict {
        if self.flavor == Flavor::Dev {
            return BuildVerdict::Accepted;
        }
        let Some(build) = self.builds.iter().find(|b| b.version == ext_version) else {
            return BuildVerdict::UnknownVersion(ext_version.to_string());
        };
        let (Some(want), Some(dir)) = (build.sha256, installed_dir) else {
            return BuildVerdict::Accepted;
        };
        match hash_extension_dir(dir) {
            Ok(got) if got.eq_ignore_ascii_case(want) => BuildVerdict::Accepted,
            // A folder that was found but cannot be hashed, or hashes differently, fails closed.
            // Only a missing folder (`installed_dir` is `None`) or a build without a recorded
            // hash falls back to the version check.
            Ok(_) | Err(_) => BuildVerdict::HashMismatch(ext_version.to_string()),
        }
    }
}

/// `<user data dir>/<profile dir>/Extensions/<id>/<version>_<n>/`, the newest folder for the
/// reported version, when it exists.
pub(crate) fn installed_extension_dir(
    user_data_dir: &Path,
    profile_dir: &str,
    extension_id: &str,
    version: &str,
) -> Option<PathBuf> {
    if profile_dir.contains(['/', '\\']) || extension_id.contains(['/', '\\']) {
        return None;
    }
    let base = user_data_dir
        .join(profile_dir)
        .join("Extensions")
        .join(extension_id);
    let prefix = format!("{version}_");
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(base).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(n) = rest.parse::<u32>() else { continue };
        if best.as_ref().map(|(b, _)| n > *b).unwrap_or(true) {
            best = Some((n, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// SHA-256 over the package: every file in sorted path order, `manifest.json` without its `key` and
/// `update_url`, `_metadata/` skipped. Each file contributes `path NUL sha256(content)`.
pub(crate) fn hash_extension_dir(dir: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    let mut outer = Sha256::new();
    for rel in files {
        let bytes = std::fs::read(dir.join(&rel))?;
        let bytes = if rel == "manifest.json" {
            strip_manifest(&bytes)
        } else {
            bytes
        };
        outer.update(rel.as_bytes());
        outer.update([0]);
        outer.update(Sha256::digest(&bytes));
    }
    Ok(outer
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            if dir == root && entry.file_name() == "_metadata" {
                continue;
            }
            collect(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    Ok(())
}

/// `manifest.json` without `key` and `update_url`, written canonically: object keys sorted
/// recursively, no whitespace. Independent of serde_json's `preserve_order` feature (which
/// feature unification can switch on) so the JS release script computes the same bytes
/// (`canonicalManifest` in extensions/zptah-browser/scripts/release.mjs; shared vectors in
/// `shared/manifest-canon-vectors.json`).
pub(crate) fn strip_manifest(bytes: &[u8]) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return bytes.to_vec();
    };
    if let Some(map) = value.as_object_mut() {
        map.remove("key");
        map.remove("update_url");
    }
    let mut out = String::new();
    write_canonical(&value, &mut out);
    out.into_bytes()
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}
