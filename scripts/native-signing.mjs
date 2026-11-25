// Release signing identity for zptah (macOS).
//
// Releases are signed with a stable self-signed certificate, not ad-hoc, so the
// designated requirement (DR) stays `identifier "app.zptah.desktop" and
// certificate leaf = H"<sha1>"` across updates and macOS TCC keeps grants such as
// Full Disk Access. Setup and recovery: docs/native/release-flow.md, "Stable signing identity".

export const SIGNING_IDENTITY_ENV = "MONOPTAH_NATIVE_SIGNING_IDENTITY";
export const SIGNING_KEYCHAIN_ENV = "MONOPTAH_NATIVE_SIGNING_KEYCHAIN";

const HELP =
  "Create it once with scripts/setup-native-signing-identity.sh (or restore it from the backup with --import), " +
  `then export ${SIGNING_IDENTITY_ENV}=<SHA-1>. See docs/native/release-flow.md, "Stable signing identity".`;

// Lines look like: `  1) <40 hex> "Common Name" (CSSMERR_TP_NOT_TRUSTED)`. Untrusted is fine.
export function listedIdentities(findIdentityOutput) {
  const found = [];
  for (const line of String(findIdentityOutput).split("\n")) {
    const m = /^\s*\d+\)\s+([0-9A-Fa-f]{40})\s+"(.*)"/.exec(line);
    if (m) found.push({ sha1: m[1].toUpperCase(), name: m[2] });
  }
  return found;
}

export function identityListed(identity, findIdentityOutput) {
  const wanted = identity.trim();
  return listedIdentities(findIdentityOutput).some((i) =>
    /^[0-9A-Fa-f]{40}$/.test(wanted) ? i.sha1 === wanted.toUpperCase() : i.name === wanted,
  );
}

export const findIdentityArgs = (keychain) => [
  "find-identity",
  "-p",
  "codesigning",
  ...(keychain ? [keychain] : []),
];

// Resolves the configured identity to one certificate SHA-1 (uppercase).
// `run(command, args)` returns { status, stdout, stderr }. Returns { sha1 } or
// { problem } (unset, not listed, or a common name that matches several certificates).
export function resolveSigningIdentity(env, run) {
  const identity = (env[SIGNING_IDENTITY_ENV] ?? "").trim();
  if (!identity) return { problem: `${SIGNING_IDENTITY_ENV} is not set, so the release cannot be signed. ${HELP}` };
  const keychain = (env[SIGNING_KEYCHAIN_ENV] ?? "").trim();
  const where = keychain || "the keychain search list";
  const listed = run("security", findIdentityArgs(keychain));
  const all = listed.status === 0 ? listedIdentities(listed.stdout ?? "") : [];
  const hash = /^[0-9A-Fa-f]{40}$/.test(identity);
  const matches = [
    ...new Set(all.filter((i) => (hash ? i.sha1 === identity.toUpperCase() : i.name === identity)).map((i) => i.sha1)),
  ];
  if (matches.length === 0)
    return { problem: `The signing identity "${identity}" is not in ${where} (security find-identity -p codesigning). ${HELP}` };
  if (matches.length > 1)
    return {
      problem: `The common name "${identity}" matches ${matches.length} identities in ${where} (${matches.join(", ")}). Set ${SIGNING_IDENTITY_ENV} to the SHA-1 of the one to use.`,
    };
  return { sha1: matches[0] };
}

// Returns a problem message, or null when the identity is configured and listed.
export const signingIdentityProblem = (env, run) => resolveSigningIdentity(env, run).problem ?? null;

// Entitlements the release signature must carry, exactly (assets/native/Entitlements.plist).
// Audit: the app has no dlopen of plugins or vendor dylibs (system frameworks only), no
// DYLD_* use, and no microphone, camera, screen capture, Apple Events target or Keychain
// access, so only the two memory entitlements are needed. Adding one needs a new audit.
export const REQUIRED_ENTITLEMENTS = [
  "com.apple.security.cs.allow-jit",
  "com.apple.security.cs.allow-unsigned-executable-memory",
];

// Pure: problem message for the output of `codesign -d --entitlements :-`, or null.
export function entitlementsProblem(entitlementsText) {
  const text = String(entitlementsText);
  const keys = [...text.matchAll(/<key>([^<]+)<\/key>\s*<true\/>/g)].map((m) => m[1]).sort();
  const allKeys = [...text.matchAll(/<key>([^<]+)<\/key>/g)].map((m) => m[1]).sort();
  const expected = [...REQUIRED_ENTITLEMENTS].sort();
  if (JSON.stringify(keys) !== JSON.stringify(expected) || JSON.stringify(allKeys) !== JSON.stringify(expected))
    return `The signed entitlements are not exactly ${expected.join(", ")}: ${text.trim() || "(none)"}`;
  return null;
}

// Pure: problem message for the output of `codesign -d -r-` and `codesign -dv`, or null.
// The requirement must be exactly the stable form for this certificate: no other
// leaf hash and no extra `or` clauses. The code directory flags must be exactly the
// hardened runtime (0x10000): the self-signed signature is not ad-hoc (0x2) and has no
// other flag.
export function signatureProblem(requirementText, detailsText, sha1) {
  if (/^Signature=adhoc$/m.test(detailsText)) return "The packaged app is ad-hoc signed";
  if (!/flags=0x10000\(runtime\)/.test(detailsText))
    return `The packaged app is not signed with the hardened runtime (expected flags=0x10000(runtime)): ${detailsText.trim() || "(empty)"}`;
  const expected = `designated => identifier "app.zptah.desktop" and certificate leaf = H"${sha1.toLowerCase()}"`;
  const lines = requirementText.split("\n").map((l) => l.trim());
  if (!lines.includes(expected))
    return `The designated requirement is not exactly the expected stable form (${expected}): ${requirementText.trim() || "(empty)"}`;
  if (lines.some((l) => l.startsWith("designated =>") && l !== expected))
    return `The designated requirement has extra clauses: ${requirementText.trim()}`;
  return null;
}
