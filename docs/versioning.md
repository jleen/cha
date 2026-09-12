---
author: Claude (Anthropic)
ai-generated: true
---

> **Provenance:** Written by Claude (Anthropic) while doing the work it
> describes. Reviewed in the normal course of review, but not audited line by
> line — treat specific numbers, paths, and version pins as claims to verify
> rather than guarantees.
>
> See [docs/README.md](README.md).

# Versioning: one number, in `[workspace.package]`

**To bump the version, edit `version` under `[workspace.package]` in the root
`Cargo.toml`. That is the only place.** All four crates inherit it with
`version.workspace = true`, and everything downstream chains off that:

- **The Tauri bundle version** — `tauri.conf.json` deliberately has **no**
  `version` field. Tauri falls back to `CARGO_PKG_VERSION` from
  `cha-gui/src-tauri/Cargo.toml` (see `tauri-codegen`'s `context.rs`). Don't
  "helpfully" add the field back; that reintroduces a second source of truth.
  Tauri's own docs recommend the opposite direction, which is right for a
  single-crate app and wrong for this four-crate workspace.
- **Android's `versionName`/`versionCode`** — derived from the crate version
  into the git-ignored `gen/android/app/tauri.properties`, regenerated per build.
- **The container image tag and the GitHub release name** — derived from the git
  tag, *not* from the source. The `verify-version` job in `release.yml` gates
  both release jobs on the tag matching this version, so a `v0.6.0` tag on a
  0.5.0 tree fails before anything reaches GHCR.
- **iOS's `CFBundleShortVersionString`/`CFBundleVersion`** — indirected through
  build settings. `gen/apple/cha-gui_iOS/Info.plist` holds `$(MARKETING_VERSION)`
  and `$(CURRENT_PROJECT_VERSION)` rather than literals, and
  [`ios-testflight.yml`](../.github/workflows/ios-testflight.yml) sets both on the
  `xcodebuild` command line — the marketing version parsed out of this same
  `Cargo.toml`, the build number from the run number. Command-line build settings
  outrank the project, so **nothing that ships can drift.** The literals still
  present in `project.yml` and `project.pbxproj` are fallbacks for *local* Xcode
  and `tauri ios dev` builds only; they're cosmetic and don't need chasing on
  every bump — with one exception, a hand-driven Organizer archive, which does
  read them (see "iOS, remote tester → TestFlight" below). (Why a separate build
  number at all: App Store Connect rejects a
  repeat of a `(CFBundleShortVersionString, CFBundleVersion)` pair, so re-uploading
  a fixed build of the same version needs a number that only ever increases.)
  Keep `project.yml` and `Info.plist` in sync by hand — nothing runs XcodeGen in
  CI, and `tauri ios init` would clobber the hand edits in `project.yml` (the
  `PATH` preBuildScript, `ARCHS`, `LIBRARY_SEARCH_PATHS`).

One accepted cost of dropping `version` from `tauri.conf.json`: a macOS
`tauri dev` run's embedded `Info.plist` no longer gets
`CFBundleShortVersionString` (that codegen branch is gated on the field being
present, and on `dev` — release bundles are unaffected).

---

Back to [AGENTS.md](../AGENTS.md).
