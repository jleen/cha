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

# Mobile (iOS + Android, Tauri v2)

The same crate and the same `cha-gui/ui` front end ship to five platforms. Mobile
is deliberately stripped down: **embedded dictionary only** (no config dir, no
"Open Dictionary Folder"), **no multiwindow**, and the pattern-syntax cheat sheet
reached through an in-page sheet instead of a menu. Desktop rendering and behavior
are unchanged — every mobile addition is behind a cfg seam, a `.mobile` body
class, or a CSS rule that is a literal no-op on desktop.

- **The lib/bin split.** `run()` in [`lib.rs`](../cha-gui/src-tauri/src/lib.rs) is
  the single entry point for all platforms — the desktop
  [`main.rs`](../cha-gui/src-tauri/src/main.rs) is a 5-line shim that only holds
  `windows_subsystem` (a bin-crate attribute) and calls `cha_gui_lib::run()`; on
  mobile the platform shell calls `run()` via `#[cfg_attr(mobile,
  tauri::mobile_entry_point)]`. `Cargo.toml` has `[lib] name = "cha_gui_lib"`
  with `crate-type = ["staticlib", "cdylib", "rlib"]` — staticlib for iOS, cdylib
  for Android, rlib for the desktop bin. The `_lib` suffix avoids a Windows
  bin/lib artifact collision (cargo#8519), and this repo ships Windows, so keep
  it. `crate-type` can't be cfg-gated (hence `--bins` for a fast desktop build).
- **`#[cfg(desktop)] mod desktop;` is the one seam.** Everything mobile doesn't
  have — the menu bar, extra windows, the Pattern Syntax window, the config-dir
  dictionary, the file-manager shell-out — lives in
  [`desktop.rs`](../cha-gui/src-tauri/src/desktop.rs). Because the module isn't
  compiled on mobile, nothing in it can be dead code there; because it's all
  reachable on desktop, nothing is dead there either. **Neither platform needs a
  single `#[allow(dead_code)]`.** A new desktop-only feature goes *in that
  module*, not behind a fresh inline `#[cfg]` in `lib.rs`. The only unavoidable
  straddler is `load_dict`, whose two `#[cfg]` lines are commented as such.
- **`generate_handler![]` takes per-entry `#[cfg]`.** The mobile handler list
  omits `desktop::open_dict_dir` via `#[cfg(desktop)]` right inside the macro
  (tauri-macros re-emits the attr onto the generated match arm). This keeps one
  handler list instead of two divergent copies. If it ever breaks, the fallback
  is two `#[cfg]`'d `.invoke_handler(...)` calls.
- **`platform` is the front end's only source of platform truth.** Its body is
  `if cfg!(mobile) { "mobile" } else { "desktop" }` — an *expression*, so one
  command serves both platforms — and `cha-web` returns `"web"` from its own
  handler. Those three strings are the only values the front end understands. It
  returns a string rather than the old `is_mobile` boolean because web wants the
  mobile help affordance (a browser tab has no menu bar we control) while not
  being mobile, which a boolean can't express. The front end (`init()` in
  [`main.js`](../cha-gui/ui/main.js)) awaits it once at startup and drives the help
  button, the Ctrl+N handler, the submission model, and the
  "Open Dictionary Folder" gate off it. **Don't** UA-sniff (iPadOS WKWebView
  reports ambiguously) and **don't** infer platform from
  `@media (pointer: coarse)` (a touch laptop matches it — that's a touch
  question, not a platform question). Also note the
  `window.__TAURI__.webviewWindow` destructure lives *inside* the desktop branch,
  not at top level, so a mobile bundle that omits it can't throw and kill the
  whole script.
- **The help sheet's iframe must be navigated with `location.replace`, never by
  assigning `src`.** Assigning `src` commits asynchronously and adds an entry to
  the *joint session history*, which lands after `openHelp`'s `pushState` (that
  runs synchronously on the same tick). `closeHelp`'s `history.back()` then
  returns to the entry where the iframe was still `about:blank`, so the sheet is
  blank on every subsequent open until a full page reload. `replace()`
  contributes no history entry, which removes the ordering problem rather than
  racing it. Confirmed in headless Firefox — with `src` the iframe reads
  `about:blank` immediately after the first close; with `replace` it keeps its
  content across open/close/open. `openHelp` navigates on *every* open rather
  than caching a "already loaded" flag: the sheet is two pages now (the cheat
  sheet's footer links to `license.html`) and the iframe stays parked wherever
  the user left it, so without the re-navigation the ? button would sometimes
  open the licenses. Keep using `replace` for that — assigning `src` puts the
  bug above straight back.
- **Transport is chosen separately from platform, and the distinction matters.**
  [`transport.js`](../cha-gui/ui/transport.js) sets `window.chaInvoke` to either
  Tauri's `invoke` or an HTTP `POST /api/<command>`, deciding by testing for
  `window.__TAURI__`. That looks like the UA-sniffing the rule above forbids and
  isn't: it asks "is the IPC bridge present in *this document*", a directly
  observable fact about how the page loaded, not a guess about the machine. The
  two questions are genuinely independent — a desktop browser hitting `cha-web`
  has the HTTP transport and is not a phone. Keep them separate.
  The shim must reject with a **bare string**, not an `Error`: Tauri rejects with
  the command's `Err` value and `main.js` renders failures via `String(e)`, so an
  `Error` would render as "Error: msg" on web only. That contract is pinned by
  [`ui/tests/`](../cha-gui/ui/tests/) — run `cha-gui/ui/tests/run.sh`. It uses
  whatever JS engine is around (macOS ships JavaScriptCore; node works too) and
  **skips with exit 0 when neither is present**, so it can never fail a build. No
  CI job invokes it today; run it by hand when touching `transport.js`.
- **Mobile is embedded-only by construction, and that's enforced, not hoped.**
  `build.rs` hard-errors on an `android`/`ios` target with no `words.txt` (via
  `CARGO_CFG_TARGET_OS`). On mobile `dict_status` therefore can't return a
  message (the embedded list is always non-empty), so the empty-dictionary notice
  is unreachable there. The notice's "Open Dictionary Folder" button is
  nonetheless **explicitly gated on `platform === "desktop"`** now, rather than
  relying on that unreachability: `open_dict_dir` is a `#[cfg(desktop)]` command,
  and on web the notice *is* reachable in principle while the dictionary lives on
  a server the user can't browse. Adding user lists on mobile would need a
  file-picker plugin and a real design — don't half-do it.
- **Mobile CSS is additive by construction.** In
  [`styles.css`](../cha-gui/ui/styles.css), `env(safe-area-inset-*)` is `0px` and
  `100dvh == 100vh` on desktop, so the safe-area/viewport rules ship
  unconditionally and cost desktop nothing — no class, no cfg, no first-paint
  flash. `init()` sets the platform name as a body class (`desktop` / `mobile` /
  `web`) and `platform` gates only real behavior (the help button, the Ctrl+N
  handler, the submission model). Keep it that way: a rule that needs `.mobile`
  to *avoid* breaking desktop is written wrong. The one class-scoped rule today
  is `body.web`'s `max-width`, which exists because a browser window is far wider
  than the app's 720px — additive, and invisible to the two app targets. `viewport-fit=cover` on the viewport
  meta is required for the insets to be non-zero and is a desktop no-op.
- **`#pattern` must stay ≥16px** (it's 18px). iOS zooms the page when a focused
  `<input>` is under 16px, and the zoom doesn't cleanly undo. This looks like a
  harmless tidy-up and isn't.
- **Result rows (`.word`) are deliberately not touch targets.** They're
  non-interactive text; 44px rows would cost ~45% of the visible words for no
  gain. The only 44px targets are `#help`/`#help-close`. If rows ever become
  tappable (copy-on-tap), *that's* when the sizing question opens.
- **Pattern help on mobile reuses the desktop file verbatim.** The `?` button
  opens [`pattern-syntax.html`](../cha-gui/ui/pattern-syntax.html) — the very page
  the desktop Help menu opens in a window — inside a full-screen `<iframe>` sheet
  (`#help-sheet`). It's pure static HTML with no JS/Tauri, so it drops into the
  iframe unmodified: one source of truth, zero duplication. The sheet container
  carries the safe-area padding because an iframe can't see its parent's `env()`
  insets. `openHelp` pushes a history entry so Android's hardware **Back closes
  the sheet, not the app** (verified on the emulator); ✕ and Escape also close it.
  The cheat sheet's footer link to `license.html` navigates *inside* the iframe,
  which adds a nested history entry — so from the license page Back returns to
  the cheat sheet first and only then closes the sheet. Reopening always starts
  at the cheat sheet regardless of where the user left off (see the
  `location.replace` note above).
- **`gen/schemas/` is git-ignored; `gen/android` and `gen/apple` are committed.**
  Only the ACL schemas regenerate per build; the Xcode and Gradle projects from
  `tauri {ios,android} init` are one-shot and hold the Kotlin activity, plists,
  and mobile icons — `android init` isn't reproducible enough to regenerate on a
  clean checkout. **Never `rm -rf gen/`.** The generated trees carry their own
  `.gitignore`s for build outputs (`build/`, `.gradle/`, `Pods/`, `Externals/`,
  `local.properties`, `jniLibs/**/*.so`); sanity-check `git status` after an init.
- **Mobile icons: the scratch-dir recipe, never `tauri icon` in place.** In-place
  it would clobber the hand-packed `icons/icon.ico` (and `build.rs` tracks that by
  mtime, so a touch-and-revert triggers a misleading rebuild). Instead, source the
  true 1024px master out of the icns and send everything to a scratch dir, then
  copy only the mobile outputs:
  ```
  iconutil -c iconset icons/icon.icns -o "$S/cha.iconset"
  cargo tauri icon "$S/cha.iconset/icon_512x512@2x.png" -o "$S/out"
  cp "$S/out/ios/"*.png gen/apple/Assets.xcassets/AppIcon.appiconset/   # keep the generated Contents.json
  rsync -a "$S/out/android/" gen/android/app/src/main/res/
  ```
  This can't damage `icons/` even if you forget the follow-up. Note the Android
  **adaptive** foreground is derived from the square icon and Android masks/crops
  ~25% off the edges, so 茶 loses its outer strokes — the mechanical output is a
  starting point; a proper foreground (respecting the 66/108 safe zone, via
  `tauri icon --android_fg/--android_bg`) wants a hand pass.

## Mobile toolchain and driving a device

One-time setup on macOS: Xcode + `brew install cocoapods xcodegen`; Android Studio
or `brew install --cask android-commandlinetools` plus `sdkmanager` for
`platform-tools`, `platforms;android-34`, `build-tools;34.0.0`, and an `ndk;…`;
JDK 17 or 21 (**not** 24 — Android Gradle rejects it); `rustup target add` the 3
iOS + 4 Android targets. Export `ANDROID_HOME`, `NDK_HOME`, and a JDK-21
`JAVA_HOME`. `tauri android init` reads `[lib]` from `Cargo.toml`, so do the
lib/bin split first.

```
cargo tauri ios dev "iPhone 17"           # simulator; --release to judge feel
cargo tauri android build --debug --apk --target aarch64   # then adb install/monkey
```

A freshly-booted Android emulator under heavy host load throws "Process system
isn't responding" (that's the emulator's own system_server, not the app); free
CPU and relaunch with `am start -n org.saturnvalley.cha/.MainActivity`. `eprintln!`
(which the code already uses) lands in `adb logcat` / `xcrun simctl … log stream`,
so it's the zero-dependency way to time `load_dict` if a phone ever shows a blank
startup stall — currently it doesn't, so the parse stays inline in `setup()` and
`dict_status` stays sync. If that changes, moving the parse off-thread means
`dict_status` must become `(async)` too, or it blocks the event loop.

## Test deployment to a real device

**The two platforms are not symmetric.** Android lets you build a self-signed APK
and hand it to anyone. iOS binds every install to signing that authorizes a
specific device or an App Store channel — there is no sideload-an-`.ipa`
equivalent, and remote testing effectively requires the paid Apple Developer
Program ($99/yr).

**iOS, cabled local device (free, no paid account, your own phone only).** Good
for a quick real-device smoke test. A free "Personal Team" signs apps that run
only on a device cabled to (or paired with) your Mac, expire after 7 days, and
can't use most entitlements — Cha needs none, so it's fine.
1. Plug in the iPhone, unlock, tap **Trust This Computer**, enter the passcode.
2. `cargo tauri ios open` → Xcode → target → **Signing & Capabilities** → check
   *Automatically manage signing* → **Team** → *Add an Account* (your Apple ID) →
   pick the Personal Team. Xcode writes `DEVELOPMENT_TEAM` into the **pbxproj**,
   which XcodeGen *regenerates from `project.yml`* on the next `cargo tauri ios`
   command — so that edit isn't durable and would also commit your personal team
   id. Move the value instead into **`Signing.local.xcconfig` at the repo root**
   (git-ignored) as `DEVELOPMENT_TEAM = XXXXXXXXXX`. `gen/apple/Signing.xcconfig`
   (committed, carrying no id) `#include?`s it by a relative `../../../../` climb
   to the root, and `project.yml` references that xcconfig — so the team survives
   regeneration, never lands in git, and a clone without the local file still
   builds for the Simulator (which needs no signing). It's kept at the root, not
   beside `Signing.xcconfig`, so it's visible and hard to lose; the four `../`
   must stay in sync with `gen/apple`'s depth if the project layout moves.
3. On the phone, enable **Developer Mode**: Settings → Privacy & Security →
   Developer Mode → on → restart. (Required on iOS 16+ to run dev-signed apps;
   the toggle only appears after a dev build has been targeted at the device.)
4. `cargo tauri ios dev "<iPhone name>"` (it lists connected devices). First launch
   may need Settings → General → VPN & Device Management → *Developer App* → Trust.
5. Re-run to refresh before the 7-day signature expires.

**iOS, building from Xcode's GUI.** Xcode launched from the Dock/Finder runs
with a minimal launchd `PATH` that lacks `~/.cargo/bin`, so the "Build Rust Code"
phase fails with *"Cargo: command not found"* — even though `cargo tauri ios …`
works in a terminal (whose shell `PATH` has it). The fix lives in
`gen/apple/project.yml`'s `preBuildScripts`, which prepends
`export PATH="$HOME/.cargo/bin:$PATH"` before calling `cargo tauri ios
xcode-script`. Keep it there (XcodeGen bakes it into the pbxproj on regeneration);
without it, only terminal builds work. For a standalone on-device build that
survives unplugging, prefer `cargo tauri ios dev --release --no-watch "<device>"`
— it installs a release build directly and sidesteps `ios run`'s broken
IPA-export step (`Couldn't load -exportOptionsPlist … no such file`).

**iOS, remote tester → TestFlight (paid).** The normal path is CI: run the
**iOS TestFlight** workflow from the Actions tab (manual trigger only; see
"Release signing and mobile CI" below for the one-time Apple setup and the
secrets). It archives, signs, exports, and uploads; uncheck `upload` to get just
the IPA as an artifact. You do **not** need to bump the version to re-upload — the
build number comes from the run number, and only the `(version, build)` pair has
to be unique.

Adding testers is still web-UI work in App Store Connect: **internal** testers
must be members of your ASC team but need no review and appear immediately;
**external** testers can be any email address (or a public link, up to 10,000),
but the first build sent to an external group goes through a one-time Beta App
Review. Builds expire after 90 days. Export compliance is pre-answered by
`ITSAppUsesNonExemptEncryption=false` in `Info.plist`, so builds don't park in
"Missing Compliance" — the app is fully offline and that answer stays true only
as long as it is.

The manual fallback, if CI is broken or you want to watch it happen: `cargo tauri
ios open`, then Product → Archive → Distribute → TestFlight in the Xcode
Organizer. `gen/apple/ExportOptions.plist` starts as `method: debugging` and is
rewritten by `cargo tauri ios build --export-method` — don't hand-edit it, and
note the CI workflow deliberately ignores it and writes its own into `RUNNER_TEMP`.
**On this path you must set the version by hand**: a local archive reads the
`MARKETING_VERSION`/`CURRENT_PROJECT_VERSION` literals in `project.pbxproj`,
which CI normally overrides and which nothing keeps current. Left alone they
produce a stale marketing version and a build number of `1`, which App Store
Connect rejects as a duplicate. Bump both in the Xcode target's build settings
before archiving (and don't commit the bumped build number — it's a CI counter).

**iOS, TestFlight → App Store.** No rebuild and no CI change: TestFlight builds
*are* App Store builds. In App Store Connect, create the version, pick an already-
uploaded build, and submit. What's missing is listing material — screenshots
(iPhone 6.9" **and** 13" iPad, because `TARGETED_DEVICE_FAMILY = "1,2"`), the
privacy questionnaire ("Data Not Collected" — the app has no network), a live
privacy-policy URL, age rating, and category. Full App Review, not the beta kind.
Guideline 4.2 (minimum functionality) is the realistic risk for a single-purpose
utility, which is why the shipped build must carry the full `words.txt`.

**Android, remote tester → signed APK.** Release signing is wired into the build
(see the next section), so `cargo tauri android build --apk` emits a *signed*,
installable release APK directly — the output is
`gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`.
`adb install` it or send the file. **Updates must keep the same signing key and a
higher `versionCode`** (crate-version-derived, in the git-ignored
`gen/android/app/tauri.properties`), or Android refuses the install. Scaling past
one tester is Play Console internal testing, which wants the **AAB** (`--aab`).

## Release signing and mobile CI

**Android signing lives in Gradle, driven by a git-ignored properties file.**
[`gen/android/app/build.gradle.kts`](../cha-gui/src-tauri/gen/android/app/build.gradle.kts)
has a `signingConfigs { create("release") { … } }` block (added by hand — this
file is generated once and then owned by us, *unlike* the iOS pbxproj) that reads
`rootProject.file("keystore.properties")`. Tauri's convention uses a **single
`password`** for both the store and the key, plus `keyAlias` and `storeFile` —
not separate store/key passwords. The casts are nullable (`as String?`) and
`storeFile` is guarded, so a build with **no** `keystore.properties` (a plain
debug build, or a fresh clone) still works instead of throwing; only release
signing goes unpopulated.
- `gen/android/keystore.properties` is **git-ignored** (by `gen/android/.gitignore`)
  and holds `password`/`keyAlias=upload`/`storeFile=<abs path>`. The `.jks` lives
  **outside the repo** (`~/keystores/cha-upload.jks`); never commit either.
- **Generate the key once:** `keytool -genkeypair -v -keystore
  ~/keystores/cha-upload.jks -keyalg RSA -keysize 2048 -validity 10000 -alias
  upload`. The DN fields (CN/OU/O/…) are cosmetic — Android/Play validate only the
  key's algorithm, validity, and cross-update consistency, never the DN text.
  **Losing the password or the `.jks` means you can never update the app** for
  existing installs. Verify a build with
  `build-tools/…/apksigner verify --print-certs <apk>`.
- **The real release risk is R8, not signing.** `release` has
  `isMinifyEnabled = true`; a signed APK that *builds* can still crash if proguard
  strips Tauri/webview classes. Always install-and-run the release APK, don't just
  build it. (Verified clean with the current Tauri proguard rules.)

**Mobile CI is [`.github/workflows/mobile.yml`](../.github/workflows/mobile.yml)**,
separate from the desktop `release.yml`. `workflow_dispatch` build-checks both
platforms and uploads artifacts; a `mobile-v*` tag additionally attaches the
signed Android APK + AAB to a (draft) GitHub release.
- **Android job** (`ubuntu-latest`): setup-java 17 → setup-android → `sdkmanager
  "ndk;<NDK_VERSION>"` → rust-toolchain with the 4 android targets → cargo-binstall
  tauri-cli → decode `ANDROID_KEYSTORE_BASE64` + write `keystore.properties` from
  secrets → `cargo tauri android build --apk --aab`.
- **iOS job** (`macos-latest`): **build-check only** — `cargo build -p cha-gui
  --lib --target aarch64-apple-ios` cross-compiles the shared library with **no
  Xcode archive, no signing, no secrets**, so this workflow stays runnable by
  anyone with a clone. `cargo tauri ios build` was tried here first but it always
  *archives* (device), which needs a signing team — so it fails on a runner
  without one, and only "worked" locally because this Mac has a cert. The
  cross-compile catches the breakage that matters (the shared Rust code building
  for iOS), is arch-agnostic, and needs macOS only because the iOS SDK is
  Xcode-only. **Signed iOS distribution is a separate workflow** (below), kept
  apart so an Android run never depends on Apple secrets.
- **`words.txt` in CI:** nothing to do. It's committed, freely redistributable,
  and `actions/checkout` puts it at the repo root where `build.rs` looks. The old
  `WORDS_URL`-or-`ci/words-stub.txt` "materialize" step is **gone** — it existed
  when the list was git-ignored, and once the list was committed it actively
  overwrote the real dictionary with a 2k-word placeholder.
- **Secrets to add now** (repo Settings → Secrets and variables → Actions):
  `ANDROID_KEYSTORE_BASE64` (`base64 -i ~/keystores/cha-upload.jks | pbcopy`),
  `ANDROID_KEY_PASSWORD`, `ANDROID_KEY_ALIAS` (=`upload`). The Android job hard-fails
  fast if `ANDROID_KEYSTORE_BASE64` is missing rather than shipping an unsigned APK.

**iOS release signing is
[`.github/workflows/ios-testflight.yml`](../.github/workflows/ios-testflight.yml)**,
`workflow_dispatch` only — no tag trigger, no push trigger. It's the one workflow
that holds the distribution certificate, and a stray run burns a build number.
Inputs: `upload` (default on; uncheck to stop after the IPA artifact) and
`build_number` (default: the run number).

- **Manual signing, not automatic.** The job imports an `Apple Distribution`
  `.p12` into a throwaway keychain and installs a downloaded App Store
  provisioning profile; nothing calls out to Apple at build time, so a build
  can't silently mint a new profile or burn one of the three cert slots. It reads
  the profile's **`Name` out of the profile itself** rather than taking it as a
  secret — one less thing to keep in sync at the yearly renewal.
- **Signing settings go on the `xcodebuild` command line**, not in
  `Signing.xcconfig`. `project.pbxproj` sets `CODE_SIGN_IDENTITY` in the
  *target's* build settings, which outranks the xcconfig attached to that same
  target; command-line settings outrank everything. That's also how
  `MARKETING_VERSION`/`CURRENT_PROJECT_VERSION` get injected. **Don't "fix" this
  by moving it into the xcconfig** — it will silently not take effect.
- `security set-key-partition-list` after the import is load-bearing. Without it
  `codesign` blocks on a GUI keychain prompt nobody can answer and the job hangs
  until it times out.
- **Raw `xcodebuild`, not `cargo tauri ios build`** — that's what makes the
  command-line signing overrides possible. Use `-project` (there is no Pods
  workspace) and lowercase `-configuration release` (XcodeGen named the configs
  `debug`/`release`).
- **`cargo tauri ios xcode-script` cannot run on a clean machine, so CI skips
  it.** The "Build Rust Code" pre-build phase calls that command, and it is *not*
  standalone: it calls `read_options()`, which reads
  `$TMPDIR/<identifier>-server-addr` and then connects to a **WebSocket server
  that only exists while `cargo tauri ios dev|build` is running**. There is no
  flag to bypass it. On a dev machine it works because a Tauri CLI session is
  alive; under a bare `xcodebuild` on a fresh runner it panics with *"failed to
  read missing addr file …-server-addr"*. So the workflow builds the staticlib
  itself (`cargo build -p cha-gui --lib --release --target aarch64-apple-ios
  --features tauri/custom-protocol`), copies it to
  `gen/apple/Externals/arm64/release/libapp.a`, and sets
  `CHA_PREBUILT_RUST_LIB=1` on the `xcodebuild` line; the script phase checks
  that and exits 0. **Don't remove the guard from `project.yml`/`project.pbxproj`
  thinking it's dead code** — it's the only reason a signed CI build is possible.
  Only arm64 is built: `ARCHS` is `arm64` and `EXCLUDED_ARCHS[sdk=iphoneos*]`
  drops x86_64.
- **`--features tauri/custom-protocol` is mandatory on that build.** tauri-cli's
  `build_options()` pushes it onto *every* build (and `dev_options()` filters it
  *out*, which is why dev builds don't carry it) — so hand-rolling the cargo
  invocation means hand-rolling this too. Without it `generate_context!` never
  registers the asset protocol, and the installed app fails at launch with
  *"Failed to request tauri://localhost/ … did you grant local network
  permissions? That is required to reach the development server"*. That message
  is a red herring: nothing is wrong with the network or the device, the app
  simply has no embedded assets to serve. **It builds, signs, uploads, and
  passes review-side processing perfectly — the breakage only appears on a real
  install**, so there is no CI signal for it. `mobile.yml`'s build-check passes
  the same feature so it compiles the same cfg paths.
- **`gen/apple/assets` must be created before the archive.** It's a folder
  reference in Copy Bundle Resources, but it's an *empty* directory the Tauri CLI
  makes and git cannot track one — so a fresh checkout lacks it and the Resources
  phase fails with "Build input file cannot be found". The workflow `mkdir -p`s
  it. Same class of problem as the pre-build script: things `tauri ios build`
  would have arranged, which a bare `xcodebuild` must arrange for itself.
- **Not fastlane, deliberately.** `gym`/`pilot` wrap the same three commands, and
  `match`'s reason to exist is sharing certs across a team. Adopting it would put
  a Ruby toolchain into a Rust workspace that has none — no `Gemfile`, no
  `Fastfile`, CocoaPods never even run — to replace ~40 lines of YAML, and would
  add an abstraction layer between you and already-cryptic signing errors.
  Reconsider only if store metadata/screenshots start wanting version control
  (`deliver`) or testers need scripted management (`pilot`).
- **Secrets:** `APPLE_TEAM_ID`, `IOS_DIST_CERT_P12` (`base64 -i dist.p12`),
  `IOS_DIST_CERT_PASSWORD`, `IOS_PROVISION_PROFILE` (`base64 -i
  *.mobileprovision`), plus `APPLE_API_KEY`/`APPLE_API_ISSUER`/
  `APPLE_API_KEY_CONTENT` for the upload — **shared with `release.yml`'s macOS
  notarization**, which works as long as that key has App Manager access. The job
  checks for the first four up front rather than failing inside `codesign` twenty
  minutes later. Note these are **iOS-type** credentials: the `APPLE_CERTIFICATE`
  / `APPLE_SIGNING_IDENTITY` Developer ID secrets used for macOS notarization
  cannot sign iOS.
- **The provisioning profile expires after one year.** The symptom is a signing
  failure in the archive step; the fix is re-downloading it from the developer
  portal and re-pasting the secret.
- **Expect an ITMS-91053 email** ("missing API declaration") after the first
  upload — Rust std and WKWebView touch required-reason APIs. It's a warning for
  TestFlight but **blocks App Store submission**. The fix is a
  `PrivacyInfo.xcprivacy` in `gen/apple/cha-gui_iOS/` with the reason codes
  Apple's email names, then `xcodegen generate` in `gen/apple` and commit the
  regenerated `project.pbxproj` so it's bundled as a resource. Wait for the email
  rather than guessing the codes.

---

Back to [AGENTS.md](../AGENTS.md).
