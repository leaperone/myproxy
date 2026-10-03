(function () {
  "use strict";

  const api = globalThis.browser || globalThis.chrome;
  const mode = document.querySelector("#mode");
  const host = document.querySelector("#host");
  const port = document.querySelector("#port");
  const pattern = document.querySelector("#pattern");
  const action = document.querySelector("#action");
  const rules = document.querySelector("#rules");
  const stateLabel = document.querySelector("#state");
  let current = null;

  function send(message) {
    if (globalThis.browser) {
      return api.runtime.sendMessage(message).then(response => {
        if (!response || !response.ok) throw new Error(response?.error || "扩展操作失败");
        return response.state;
      });
    }
    return new Promise((resolve, reject) => {
      api.runtime.sendMessage(message, response => {
        const error = api.runtime.lastError;
        if (error) return reject(new Error(error.message));
        if (!response || !response.ok) return reject(new Error(response?.error || "扩展操作失败"));
        resolve(response.state);
      });
    });
  }

  function setStateLabel(text, error = false) {
    stateLabel.textContent = text;
    stateLabel.style.color = error ? "#ff3b30" : "";
  }

  function render() {
    mode.value = current.mode;
    host.value = current.mixedHost;
    port.value = current.mixedPort;
    rules.replaceChildren();
    for (const [index, rule] of current.rules.entries()) {
      const item = document.createElement("li");
      item.className = "rule";
      const value = document.createElement("span");
      value.textContent = rule.pattern;
      const kind = document.createElement("em");
      kind.textContent = rule.action === "direct" ? "直连" : "代理";
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = "删除";
      remove.addEventListener("click", async () => {
        current.rules.splice(index, 1);
        await save();
      });
      item.append(value, kind, remove);
      rules.append(item);
    }
    if (!current.rules.length) {
      const empty = document.createElement("li");
      empty.className = "rule";
      empty.textContent = "还没有网站规则";
      empty.style.color = "#6e6e73";
      rules.append(empty);
    }
  }

  async function save() {
    try {
      current.mode = mode.value;
      current.mixedHost = host.value.trim();
      current.mixedPort = Number(port.value);
      current = await send({ type: "saveState", state: current });
      render();
      setStateLabel("已应用");
    } catch (error) {
      setStateLabel(error.message, true);
    }
  }

  document.querySelector("#add-rule").addEventListener("submit", async event => {
    event.preventDefault();
    const value = pattern.value.trim();
    if (!value) return;
    current.rules.unshift({ pattern: value, action: action.value });
    pattern.value = "";
    await save();
  });

  document.querySelector("#add-current").addEventListener("click", async () => {
    try {
      current = await send({ type: "addCurrentRule" });
      render();
      setStateLabel("已添加");
    } catch (error) {
      setStateLabel(error.message, true);
    }
  });

  document.querySelector("#apply").addEventListener("click", save);

  document.querySelector("#reset").addEventListener("click", async () => {
    try {
      current = await send({ type: "reset" });
      render();
      setStateLabel("已恢复");
    } catch (error) {
      setStateLabel(error.message, true);
    }
  });

  send({ type: "getState" })
    .then(state => {
      current = state;
      render();
      setStateLabel("已连接");
    })
    .catch(error => setStateLabel(error.message, true));
})();
