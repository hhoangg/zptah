# Permission justifications

One entry per permission in `manifest.json` (spec 13.2). Paste into the "Privacy practices" tab of
the dashboard. `test/manifest.test.mjs` pins the permission list; changing a permission is a store
re-review, so update this file, the manifest, that test and the listing together.

## Required permissions

| Permission | Justification |
|---|---|
| `nativeMessaging` | Connects to the zptah desktop app on this computer. There is no network connection; all commands come from the local app. |
| `tabs` | Lists your tabs so a Mono can read the ones you mention, and tracks the tabs a Mono works in. |
| `tabGroups` | Keeps each Mono's tabs in its own named, colored group so you can see what it is doing. |
| `scripting` | Reads page content and performs clicks and typing in the page, only for tabs a Mono reads or works in at your request. |
| `storage` | Remembers the pairing with the zptah app, which Mono owns which tab, and your cursor settings. |
| `alarms` | Reconnects to the zptah app after the browser wakes up or the background worker restarts. |
| `webNavigation` | Finds the frames of a page to read it fully, and restores your front tab when a page a Mono works in opens a popup. |

## Host permission

| Permission | Justification |
|---|---|
| `<all_urls>` | Monos work on the sites you ask them to use. Nothing runs on a page until a Mono reads or acts on it. Each Mono has its own allowed and blocked site lists, and browser pages, the Web Store and private windows are refused. |

## Optional permission

| Permission | Justification |
|---|---|
| `debugger` (optional, requested on the pairing page or in options, never at install) | Used only for sites that ignore simulated input, for rich text editors, and for page dialogs (attached before a delete or payment step so its confirmation dialog can be read). When a tab needs it, it stays attached while the assistant works in that tab and is detached when the work ends, after two idle minutes, or when the user presses Stop. Chrome shows its debugging bar while attached. Only a fixed list of input, screenshot and dialog commands is ever sent; nothing else. |

## Not requested

`identity`, `downloads`, `cookies`, `history`, `bookmarks`, `webRequest`, `declarativeNetRequest`,
`notifications`, `sidePanel`, `activeTab`, `management`, `clipboardRead`, `clipboardWrite`,
`userScripts`. The manifest has no `externally_connectable`, no `content_scripts` and no
`web_accessible_resources`; `incognito` is `not_allowed`.

## Remote code

None. MV3 scripts only, bundled in the package. The build fails on `eval`, `new Function`,
`importScripts`, dynamic `import(`, `fetch(`, `XMLHttpRequest`, `WebSocket`, `EventSource`,
`sendBeacon`, `chrome.userScripts`, `registerContentScripts` and `setUninstallURL`.
