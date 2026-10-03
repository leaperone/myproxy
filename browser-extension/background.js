(function () {
  "use strict";

  const api = globalThis.browser || globalThis.chrome;
  const isFirefox = Boolean(globalThis.browser && globalThis.browser.proxy);
  const isSafari = !api.proxy;
  const STORAGE_KEY = "myproxyBrowserState";
  const DEFAULT_STATE = {
    mode: "smart",
    mixedHost: "127.0.0.1",
    mixedPort: 7891,
    rules: []
  };
  let state = { ...DEFAULT_STATE };

  function clone(value) {
    return JSON.parse(JSON.stringify(value));
  }

  function storageGet(defaults) {
    if (isFirefox || isSafari) {
      return api.storage.local.get(defaults);
    }
    return new Promise((resolve, reject) => {
      api.storage.local.get(defaults, result => {
        const error = api.runtime.lastError;
        if (error) reject(new Error(error.message));
        else resolve(result);
      });
    });
  }

  function storageSet(values) {
    if (isFirefox || isSafari) {
      return api.storage.local.set(values);
    }
    return new Promise((resolve, reject) => {
      api.storage.local.set(values, () => {
        const error = api.runtime.lastError;
        if (error) reject(new Error(error.message));
        else resolve();
      });
    });
  }

  function proxySettingsSet(value) {
    if (isFirefox || isSafari) {
      return api.proxy.settings.set({ value });
    }
    return new Promise((resolve, reject) => {
      api.proxy.settings.set({ value, scope: "regular" }, () => {
        const error = api.runtime.lastError;
        if (error) reject(new Error(error.message));
        else resolve();
      });
    });
  }

  function nativeMessage(message) {
    if (!api.runtime.sendNativeMessage) {
      return Promise.reject(new Error("Safari 原生桥接不可用"));
    }
    const result = api.runtime.sendNativeMessage("local.harry.myproxy", message);
    return result && typeof result.then === "function"
      ? result
      : Promise.resolve(result);
  }

  function normalizeState(raw) {
    const next = { ...DEFAULT_STATE, ...(raw || {}) };
    next.mode = ["smart", "always", "direct", "system"].includes(next.mode)
      ? next.mode
      : DEFAULT_STATE.mode;
    next.mixedHost = typeof next.mixedHost === "string" && next.mixedHost.trim()
      ? next.mixedHost.trim()
      : DEFAULT_STATE.mixedHost;
    const port = Number(next.mixedPort);
    next.mixedPort = Number.isInteger(port) && port > 0 && port < 65536
      ? port
      : DEFAULT_STATE.mixedPort;
    next.rules = Array.isArray(next.rules)
      ? next.rules
        .filter(rule => rule && typeof rule.pattern === "string")
        .map(rule => ({
          pattern: rule.pattern.trim(),
          action: rule.action === "direct" ? "direct" : "proxy"
        }))
        .filter(rule => rule.pattern)
      : [];
    return next;
  }

  function escapeRegex(value) {
    return value.replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, ".*")
      .replace(/\?/g, ".");
  }

  function ruleRegex(pattern) {
    let value = pattern.trim().toLowerCase();
    if (!value) return null;
    if (!value.includes("://") && !value.includes("/")) {
      value = value.replace(/^\*\./, "");
      return new RegExp(`(^|\\.)${escapeRegex(value)}$`, "i");
    }
    return new RegExp(escapeRegex(value), "i");
  }

  function matches(rule, url, host) {
    const regex = ruleRegex(rule.pattern);
    if (!regex) return false;
    const normalizedUrl = String(url || "").toLowerCase();
    const normalizedHost = String(host || "").toLowerCase().replace(/\.$/, "");
    return regex.test(normalizedHost) || regex.test(normalizedUrl);
  }

  function decision(url, host, current = state) {
    if (current.mode === "direct") return "direct";
    if (current.mode === "system") return "system";
    const matched = current.rules.find(rule => matches(rule, url, host));
    if (current.mode === "always") {
      return matched && matched.action === "direct" ? "direct" : "proxy";
    }
    return matched && matched.action === "proxy" ? "proxy" : "direct";
  }

  function proxyResult(url, host) {
    const result = decision(url, host);
    if (result === "system") return { type: "system" };
    if (result === "direct") return { type: "direct" };
    return { type: "http", host: state.mixedHost, port: state.mixedPort };
  }

  function pacScript(current) {
    const serialized = JSON.stringify({
      mode: current.mode,
      mixedHost: current.mixedHost,
      mixedPort: current.mixedPort,
      rules: current.rules
    }).replace(/</g, "\\u003c");
    return `
      const CONFIG = ${serialized};
      function escapeRegex(value) {
        return value.replace(/[.+^\\x24{}()|[\\]\\\\]/g, "\\\\$&")
          .replace(/\\*/g, ".*").replace(/\\?/g, ".");
      }
      function ruleRegex(pattern) {
        let value = String(pattern || "").trim().toLowerCase();
        if (!value) return null;
        if (!value.includes("://") && !value.includes("/")) {
          value = value.replace(/^\\*\\./, "");
          return new RegExp("(^|\\\\.)" + escapeRegex(value) + "$", "i");
        }
        return new RegExp(escapeRegex(value), "i");
      }
      function matches(rule, url, host) {
        const regex = ruleRegex(rule.pattern);
        if (!regex) return false;
        return regex.test(String(host || "").toLowerCase().replace(/\\.$/, ""))
          || regex.test(String(url || "").toLowerCase());
      }
      function FindProxyForURL(url, host) {
        if (CONFIG.mode === "direct") return "DIRECT";
        if (CONFIG.mode === "system") return "SYSTEM";
        const matched = CONFIG.rules.find(rule => matches(rule, url, host));
        const shouldProxy = CONFIG.mode === "always"
          ? !(matched && matched.action === "direct")
          : Boolean(matched && matched.action === "proxy");
        return shouldProxy
          ? "PROXY " + CONFIG.mixedHost + ":" + CONFIG.mixedPort
          : "DIRECT";
      }
    `;
  }

  async function apply() {
    if (isSafari) {
      await nativeMessage({ type: "browserProxyApply", state: clone(state) });
      return;
    }
    if (isFirefox) {
      await proxySettingsSet({ proxyType: state.mode === "system" ? "system" : "none" });
      return;
    }
    if (state.mode === "direct") {
      await proxySettingsSet({ mode: "direct" });
    } else if (state.mode === "system") {
      await proxySettingsSet({ mode: "system" });
    } else {
      await proxySettingsSet({ mode: "pac_script", pacScript: { data: pacScript(state) } });
    }
  }

  async function load() {
    const stored = await storageGet({ [STORAGE_KEY]: DEFAULT_STATE });
    state = normalizeState(stored[STORAGE_KEY]);
    if (!isSafari) await apply();
  }

  async function save(next) {
    state = normalizeState(next);
    await storageSet({ [STORAGE_KEY]: state });
    await apply();
    return clone(state);
  }

  async function handleMessage(message, sender) {
    switch (message && message.type) {
      case "getState":
        return clone(state);
      case "saveState":
        return save(message.state);
      case "apply":
        await apply();
        return clone(state);
      case "addCurrentRule": {
        const tab = sender && sender.tab;
        const url = tab && tab.url;
        if (!url || !/^https?:/i.test(url)) throw new Error("当前页面没有可用的网站地址");
        const parsed = new URL(url);
        const next = clone(state);
        if (!next.rules.some(rule => rule.pattern === parsed.hostname && rule.action === "proxy")) {
          next.rules.unshift({ pattern: parsed.hostname, action: "proxy" });
        }
        return save(next);
      }
      case "reset":
        return save(DEFAULT_STATE);
      default:
        throw new Error("未知扩展消息");
    }
  }

  if (isFirefox) {
    api.proxy.onRequest.addListener(details => proxyResult(details.url, details.host), {
      urls: ["<all_urls>"]
    });
  }

  api.runtime.onMessage.addListener((message, sender, sendResponse) => {
    handleMessage(message, sender)
      .then(result => sendResponse({ ok: true, state: result }))
      .catch(error => sendResponse({ ok: false, error: error.message }));
    return true;
  });

  load().catch(error => console.error("MyProxy extension initialization failed", error));
})();
