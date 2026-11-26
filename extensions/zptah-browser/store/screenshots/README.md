# Store screenshots

Not committed yet: capture these by hand for the listing (spec 13.1) and keep them out of git until
the listing is submitted (the dashboard takes them directly). Use a clean Chrome profile with no
personal tabs, a throwaway demo Mono, and only public demo sites. Never show real accounts, mail,
addresses or payment pages.

## Size and format

- 1280x800 (preferred) or 640x400, 24-bit PNG or JPEG, no alpha, full bleed, no device frame.
- One to five images; submit all five below, in this order. Capture the Vietnamese set too if the
  app UI is switched to Vietnamese (the dashboard allows per-language screenshots).
- Small promo tile (optional but recommended): 440x280. Marquee: 1400x560 (optional).

## What to capture

1. The Mono's tab group: a window with a colored, named group holding two or three tabs the Mono
   opened, and the glow and "Stop <Mono>" button on the active tab.
2. The virtual cursor: the Mono's cursor mid-click on a demo page (use a public demo form).
3. The options page (`src/options/options.html`): pairing status, the debugger fallback switch and
   the cursor settings.
4. An approval card in the zptah app: a risky step (for example a demo "Delete project" button)
   with the screenshot preview and the Allow once, Allow in this conversation and Deny buttons.
5. The zptah app Settings for a Mono: read-only versus act, and the allowed and blocked sites.

## Checks before submitting

- No emails, names, tokens or URLs with secrets visible; leave them out of the frame instead.
- The text matches the current build (capture from `dist/store` loaded through the dev flow or
  the release build, not from stale mockups).
- Add a one-line caption in the dashboard for each image.
