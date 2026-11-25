// The "never focus" invariant (spec 6.5 rule 1) as a wrapper. Everything in the worker that
// changes a tab or window goes through this object, so a request can never bring a tab or a
// window to the front. It exposes no `highlight` and no `bringToFront`.
import { ZptahError } from "./errors.js";

const NO_FOCUS = "zptah Browser never brings a tab or window to the front.";

/** @param {Record<string, unknown> | undefined} props */
function refuseTabProps(props) {
  const p = props ?? {};
  if (p.active === true || p.highlighted === true || p.selected === true) throw new ZptahError("E_INVALID", NO_FOCUS);
}

/** @param {Record<string, unknown> | undefined} props */
function refuseWindowProps(props) {
  const p = props ?? {};
  if (p.focused === true || p.drawAttention === true || p.state === "fullscreen") throw new ZptahError("E_INVALID", NO_FOCUS);
}

/**
 * @param {any} api the chrome object (or a fake)
 */
export function focusGuard(api) {
  return {
    tabs: {
      /** The only change the act path makes to a tab: its address. @param {number} tabId @param {string} url */
      navigate(tabId, url) {
        return api.tabs.update(tabId, { url });
      },
      /** @param {number} tabId @param {Record<string, unknown>} props */
      update(tabId, props) {
        refuseTabProps(props);
        return api.tabs.update(tabId, { ...props });
      },
      /** New tabs open in the background. @param {Record<string, unknown>} props */
      create(props) {
        refuseTabProps(props);
        return api.tabs.create({ ...props, active: false });
      },
      /** @param {number} tabId */
      goBack: (tabId) => api.tabs.goBack(tabId),
      /** @param {number} tabId */
      goForward: (tabId) => api.tabs.goForward(tabId),
    },
    windows: {
      /** @param {number} windowId @param {Record<string, unknown>} props */
      update(windowId, props) {
        refuseWindowProps(props);
        return api.windows.update(windowId, { ...props });
      },
    },
  };
}
