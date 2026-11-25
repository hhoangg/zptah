# zptah Browser

zptah Browser is a Chrome extension that lets a Mono work in your real Chrome profile: it is
already signed in, so the Mono can read and use the sites you use. It replaces the old Playwright
MCP backend. Design: [the spec](../superpowers/specs/2026-11-25-zptah-browser-extension.md).

## Install

The Chrome Web Store listing is not published yet. For now use the unpacked dev build:

1. From the repo root run `npm run ext:build`. It writes `extensions/zptah-browser/dist/dev`.
2. In Chrome open `chrome://extensions` and turn on Developer mode.
3. Click Load unpacked and choose `extensions/zptah-browser/dist/dev`.

The dev build is for Native Dev only. Release builds of the app connect to the Web Store build,
which will be added to the same flow once the listing exists.

## Connect and pair

1. In zptah open Settings > Browser and click Connect extension (in a Mono's Browser tab, the
   Browser connection card).
2. If the extension is missing, the app opens Chrome so you can add it. When it is installed, the
   app opens the extension's pairing page and shows "Connecting to zptah...".
3. The pairing page asks once for the optional debugger permission. Click Allow. It closes itself
   and the card shows "Connected: Chrome, profile <name>".
4. Pick the connection for each Mono that should use the browser. Several Monos can use one
   connection.

Pairing is open for 5 minutes with a single-use code. No token is shown or pasted. Disconnect in the
card removes the pairing on both sides. Connect another profile to pair a second Chrome profile.

## Levels

Set per Mono in its Browser settings:

- **Read only**: the Mono can open and read pages (snapshots, text, find, screenshots, navigation).
  It never clicks, types or changes a page.
- **Act for me** (default): the Mono can click, type and fill forms for the task. It asks first
  for risky actions (see below).

There is no "ask before every action" level.

## Approvals

Risky actions, such as payments, entering credentials, sending messages or deleting data, show an
approval card in the Mono's conversation, with a picture or text preview of the element. The copy
follows the app language (English and Vietnamese). Buttons: Allow once, Allow in this conversation
(not offered for payments and credentials) and Don't allow. Enter allows once, Escape denies. Show
tab brings the page to the front. The Mono never types passwords or one-time codes, and never works
in incognito windows.

## Stop

- While a Mono acts on a tab, the page shows a glow in the Mono's color and a "Stop <Mono>" button
  at the bottom. It stops that Mono on that tab at once and gives the tab back to you.
- The toolbar icon is the emergency stop for all Monos. Its badge counts the Monos at work.
  Resume from zptah.

## Several Monos

Each Mono gets its own Chrome tab group per window, titled with its emoji and name (for example
the frog and "Mira") and colored like the Mono. A Mono only touches tabs in its group. Tabs you
opened are yours until a Mono is given one. A Mono never steals focus.

## Troubleshooting

- **Reconnect needed**: a Mono from before the extension kept its Chrome profile but is not bound
  to a connection. Click Connect extension and bind the Mono.
- **Protocol mismatch**: the extension options page shows "Update zptah" or "Update the extension"
  and the badge shows `!`. Update whichever is older. The card button reads Try again.
- **Full Disk Access (macOS)**: optional. Without it zptah cannot read Chrome's profile list, so
  connection names do not show profile names. Connecting still works: start it from the profile
  you want. To show names, allow zptah under System Settings > Privacy & Security > Full Disk
  Access.
- **Debugger infobar**: Chrome shows "zptah Browser started debugging this browser" while the
  extension uses the debugger for sites that need it. Chrome does not let an extension hide it.
  It goes away when the task ends or on Stop. Clicking Cancel on it counts as a Stop for that tab.
- **Not connected**: check the extension is enabled in `chrome://extensions` and the card shows a
  connection; use Try again to pair again.
