#!/usr/bin/env node

const CDP_BASE = process.env.SLIME_CDP_BASE || "http://127.0.0.1:9222";
const PAGE_URL = process.env.SLIME_PAGE_URL || "http://127.0.0.1:8080";
const WINDOW_COUNT = Math.max(2, Number(process.env.SLIME_TABS || process.argv[2] || 3));
const KEEP_OPEN = process.env.SLIME_KEEP_OPEN === "1";
// Default matches shipped join-anytime (no title-screen wait for full roster).
// Set SLIME_STRICT_ROSTER=1 to require every tab to see all remotes before startGame (stress mode).
const STRICT_ROSTER = process.env.SLIME_STRICT_ROSTER === "1";
// Optional signaling override, e.g. SLIME_SIGNALING=ws://127.0.0.1:3536 for a
// local matchbox server (avoids depending on the public one during CI/smoke).
const SIGNALING_URL = process.env.SLIME_SIGNALING || "";
// Optional ICE override; SLIME_ICE=none disables STUN (host candidates only),
// which is what you want for same-machine tabs when UDP/STUN is blocked.
const ICE_URLS = process.env.SLIME_ICE || "";

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchJson(path, init = {}) {
  const res = await fetch(`${CDP_BASE}${path}`, init);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`CDP ${path} failed: ${res.status} ${text}`);
  }
  return res.json();
}

async function closePage(id) {
  try {
    await fetchJson(`/json/close/${id}`);
  } catch (_err) {
    // Best effort only.
  }
}

class CdpConnection {
  constructor(wsUrl, options = {}) {
    this.wsUrl = wsUrl;
    this.enableRuntime = options.enableRuntime || false;
    this.enablePage = options.enablePage || false;
    this.ws = null;
    this.nextId = 1;
    this.pending = new Map();
  }

  async connect() {
    this.ws = new WebSocket(this.wsUrl);
    await new Promise((resolve, reject) => {
      const onOpen = () => {
        cleanup();
        resolve();
      };
      const onError = (event) => {
        cleanup();
        reject(new Error(`WebSocket open failed for ${this.wsUrl}: ${String(event)}`));
      };
      const cleanup = () => {
        this.ws.removeEventListener("open", onOpen);
        this.ws.removeEventListener("error", onError);
      };
      this.ws.addEventListener("open", onOpen);
      this.ws.addEventListener("error", onError);
    });

    this.ws.addEventListener("message", (event) => {
      const msg = JSON.parse(event.data);
      if (!("id" in msg)) {
        return;
      }
      const pending = this.pending.get(msg.id);
      if (!pending) {
        return;
      }
      this.pending.delete(msg.id);
      if (msg.error) {
        pending.reject(new Error(msg.error.message || JSON.stringify(msg.error)));
      } else {
        pending.resolve(msg.result);
      }
    });

    if (this.enableRuntime) {
      await this.command("Runtime.enable");
    }
    if (this.enablePage) {
      await this.command("Page.enable");
    }
  }

  async command(method, params = {}) {
    const id = this.nextId++;
    const payload = JSON.stringify({ id, method, params });
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(payload, (err) => {
        if (err) {
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  async evaluate(expression) {
    const result = await this.command("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      const detail = result.exceptionDetails.text || "Runtime.evaluate exception";
      throw new Error(detail);
    }
    return result.result?.value;
  }

  async waitFor(expr, timeoutMs = 15000, label = expr) {
    const deadline = Date.now() + timeoutMs;
    let lastValue = null;
    while (Date.now() < deadline) {
      lastValue = await this.evaluate(expr);
      if (lastValue) {
        return lastValue;
      }
      await delay(150);
    }
    throw new Error(`Timed out waiting for ${label}; last=${lastValue}`);
  }

  async slime(expr) {
    return this.evaluate(`(async () => (${expr}))()`);
  }

  async close() {
    if (this.ws) {
      try {
        this.ws.close();
      } catch (_err) {
        // ignore
      }
    }
  }
}

let browserConnectionPromise = null;

async function getBrowserConnection() {
  if (!browserConnectionPromise) {
    browserConnectionPromise = (async () => {
      const version = await fetchJson("/json/version");
      const conn = new CdpConnection(version.webSocketDebuggerUrl);
      await conn.connect();
      return conn;
    })();
  }
  return browserConnectionPromise;
}

class CdpPage extends CdpConnection {
  constructor(info) {
    super(info.webSocketDebuggerUrl, { enableRuntime: true, enablePage: true });
    this.id = info.id;
    this.url = info.url;
  }

  async close() {
    await super.close();
    await closePage(this.id);
  }
}

async function createWindowPage(url) {
  const browser = await getBrowserConnection();
  const result = await browser.command("Target.createTarget", {
    url,
    newWindow: true,
    background: false,
    width: 1280,
    height: 900,
  });
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const pages = await fetchJson("/json/list");
    const page = pages.find((entry) => entry.id === result.targetId);
    if (page?.webSocketDebuggerUrl) {
      return page;
    }
    await delay(100);
  }
  throw new Error(`Could not resolve target metadata for ${result.targetId}`);
}

function parseFields(input) {
  return Object.fromEntries(
    String(input)
      .split(";")
      .map((part) => {
        const idx = part.indexOf("=");
        if (idx === -1) {
          return [part, ""];
        }
        return [part.slice(0, idx), part.slice(idx + 1)];
      })
  );
}

function parseRemotePlayers(snapshot) {
  const fields = parseFields(snapshot);
  const remote = fields.remote || "none";
  if (remote === "none") {
    return [];
  }
  return remote.split("|").map((entry) => {
    const [name, coords = "0,0"] = entry.split("@");
    const [x, y] = coords.split(",").map(Number);
    return { name, x, y };
  });
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function fmt(obj) {
  return JSON.stringify(obj, null, 2);
}

async function waitForNet(page, predicate, label, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < deadline) {
    last = await page.slime("window.slimeTest.net()");
    if (predicate(parseFields(last))) {
      return last;
    }
    await delay(400);
  }
  throw new Error(`Timed out waiting for ${label}: last=${last}`);
}

async function waitForRemotePlayerName(page, name, timeoutMs = 25000) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() + 200 < deadline) {
    last = await page.slime("window.slimeTest.players()");
    if (parseRemotePlayers(last).some((e) => e.name === name)) {
      return last;
    }
    await delay(200);
  }
  throw new Error(`Timed out waiting for remote "${name}" on player snapshot: last=${last}`);
}

async function applySignalingOverride(page) {
  // window.wasmBindings is the wasm-bindgen JS glue (handles string ABI);
  // window.slime holds raw exports whose &str params need manual encoding.
  if (SIGNALING_URL) {
    await page.slime(
      `window.wasmBindings.set_signaling_server(${JSON.stringify(SIGNALING_URL)})`
    );
  }
  if (ICE_URLS) {
    await page.slime(
      `window.wasmBindings.set_ice_servers(${JSON.stringify(ICE_URLS)}, "", "")`
    );
  }
}

async function main() {
  const pages = [];
  try {
    {
      const info = await createWindowPage(PAGE_URL);
      const page = new CdpPage(info);
      await page.connect();
      await page.waitFor(
        "typeof window.slimeTest === 'object' && typeof window.slimeTest.reloadWithProfile === 'function' && typeof window.slimeTest.players === 'function'",
        40000,
        "page 1 window.slimeTest readiness"
      );
      pages.push(page);
    }

    async function collectNetSnapshots() {
      return Promise.all(
        pages.map(async (page, idx) => {
          try {
            const line = await page.slime("window.slimeTest.net()");
            const logs = await page.slime("window.slimeTest.logs?.() || ''");
            const tail = String(logs)
              .split("\n")
              .filter(Boolean)
              .slice(-25)
              .join("\n");
            return `page${idx + 1}: ${line}${tail ? `\nlogs:\n${tail}` : ""}`;
          } catch (err) {
            return `page${idx + 1}: <unavailable: ${err.message}>`;
          }
        })
      );
    }

    const names = Array.from(
      { length: WINDOW_COUNT },
      (_, idx) => `SYNC${String.fromCharCode(65 + idx)}`
    );

    await pages[0].slime(
      `window.slimeTest.reloadWithProfile(${JSON.stringify(names[0])}, "")`
    );
    await pages[0].waitFor(
      "typeof window.slimeTest === 'object' && typeof window.slimeTest.createRoom === 'function'",
      40000,
      "page 1 reload readiness"
    );
    await applySignalingOverride(pages[0]);
    const room = await pages[0].slime("window.slimeTest.createRoom()");
    assert(typeof room === "string" && room.length >= 4, `Invalid room code: ${room}`);
    try {
      await waitForNet(
        pages[0],
        (fields) =>
          fields.room === room &&
          fields.local_peer !== "none" &&
          !String(fields.network).startsWith("error"),
        "page 1 creator local peer assignment"
      );
    } catch (err) {
      const diagnostics = await collectNetSnapshots();
      err.message += `\nDiagnostics:\n${diagnostics.join("\n")}`;
      throw err;
    }

    for (let i = 1; i < WINDOW_COUNT; i += 1) {
      const info = await createWindowPage(PAGE_URL);
      const page = new CdpPage(info);
      await page.connect();
      await page.waitFor(
        "typeof window.slimeTest === 'object' && typeof window.slimeTest.reloadWithProfile === 'function' && typeof window.slimeTest.players === 'function'",
        40000,
        `page ${i + 1} window.slimeTest readiness`
      );
      pages.push(page);
      await pages[i].slime(
        `window.slimeTest.reloadWithProfile(${JSON.stringify(names[i])}, ${JSON.stringify(room)})`
      );
      await pages[i].waitFor(
        "typeof window.slimeTest === 'object' && typeof window.slimeTest.joinSavedRoom === 'function'",
        40000,
        `page ${i + 1} reload readiness`
      );
      await applySignalingOverride(pages[i]);
      await pages[i].slime("window.slimeTest.joinSavedRoom()");
    }

    try {
      for (let i = 1; i < pages.length; i += 1) {
        await waitForNet(
          pages[i],
          (fields) =>
            fields.room === room &&
            fields.local_peer !== "none" &&
            !String(fields.network).startsWith("error"),
          `page ${i + 1} local peer assignment`
        );
      }
    } catch (err) {
      const diagnostics = await collectNetSnapshots();
      err.message += `\nDiagnostics:\n${diagnostics.join("\n")}`;
      throw err;
    }

    const expectedRemoteCount = WINDOW_COUNT - 1;
    if (STRICT_ROSTER) {
      try {
        for (let i = 0; i < pages.length; i += 1) {
          await waitForNet(
            pages[i],
            (fields) =>
              Number(fields.remote_players) === expectedRemoteCount &&
              names
                .filter((_, idx) => idx !== i)
                .every((name) => String(fields.remote_names || "").includes(name)),
            `page ${i + 1} peer convergence`
          );
        }
      } catch (err) {
        const diagnostics = await collectNetSnapshots();
        err.message += `\nDiagnostics:\n${diagnostics.join("\n")}`;
        throw err;
      }
    } else {
      await delay(1200);
    }

    for (const page of pages) {
      await page.slime("window.slimeTest.startGame()");
      await page.slime("window.slimeTest.clearEnemies()");
    }
    // The room creator's enemy state is authoritative: clearing on that page
    // every second keeps idle slimes alive through the convergence waits
    // (a death opens the respawn map and blocks the movement assertions).
    await pages[0].slime(
      "window.__slimeSafety ||= setInterval(() => window.slimeTest.clearEnemies(), 1000)"
    );

    const teleports = [
      [-240, 0],
      [0, 0],
      [240, 0],
      [0, 240],
      [0, -240],
    ];
    for (let i = 0; i < pages.length; i += 1) {
      const [x, y] = teleports[i] || [i * 120, 0];
      await pages[i].slime(`window.slimeTest.teleport(${x}, ${y})`);
    }

    await delay(1200);

    const initialNet = await Promise.all(pages.map((page) => page.slime("window.slimeTest.net()")));
    const initialPlayers = await Promise.all(
      pages.map((page) => page.slime("window.slimeTest.players()"))
    );

    if (STRICT_ROSTER) {
      initialNet.forEach((line, idx) => {
        const fields = parseFields(line);
        assert(
          Number(fields.remote_players) === expectedRemoteCount,
          `Page ${idx + 1} expected ${expectedRemoteCount} remote players, got ${fields.remote_players}: ${line}`
        );
      });
    }

    await pages[0].slime("window.slimeTest.moveFor('right', 380)");
    await delay(900);
    await pages[0].slime("window.slimeTest.phaseMove('down', 220)");
    await delay(900);

    if (!STRICT_ROSTER && pages.length >= 2) {
      await waitForRemotePlayerName(pages[1], names[0]);
    }
    if (!STRICT_ROSTER && pages.length >= 3) {
      await waitForRemotePlayerName(pages[2], names[0]);
    }

    const finalPlayers = await Promise.all(
      pages.map((page) => page.slime("window.slimeTest.players()"))
    );

    const pageAOnB = parseRemotePlayers(finalPlayers[1]).find((entry) => entry.name === names[0]);
    const pageAOnC =
      pages.length >= 3
        ? parseRemotePlayers(finalPlayers[2]).find((entry) => entry.name === names[0])
        : pageAOnB;
    assert(pageAOnB, `Page 2 did not report ${names[0]} in player snapshot: ${finalPlayers[1]}`);
    assert(
      pageAOnC,
      `Another page did not report ${names[0]} in player snapshot: ${finalPlayers[Math.min(2, pages.length - 1)]}`
    );
    assert(pageAOnB.x > -220, `Page 2 did not observe ${names[0]} moving right enough: ${fmt(pageAOnB)}`);
    assert(pageAOnC.x > -220, `Another page did not observe ${names[0]} moving right enough: ${fmt(pageAOnC)}`);
    assert(pageAOnB.y > 0, `Page 2 did not observe ${names[0]} phase-move downward: ${fmt(pageAOnB)}`);
    assert(pageAOnC.y > 0, `Another page did not observe ${names[0]} phase-move downward: ${fmt(pageAOnC)}`);

    console.log("Room:", room);
    initialNet.forEach((line, idx) => console.log(`net[${idx + 1}]: ${line}`));
    initialPlayers.forEach((line, idx) => console.log(`players-before[${idx + 1}]: ${line}`));
    finalPlayers.forEach((line, idx) => console.log(`players-after[${idx + 1}]: ${line}`));
    console.log("sync_smoke: PASS");
  } finally {
    if (!KEEP_OPEN) {
      for (const page of pages) {
        await page.close();
      }
      if (browserConnectionPromise) {
        const browser = await browserConnectionPromise;
        await browser.close();
      }
    }
  }
}

main().catch((err) => {
  console.error(`sync_smoke: FAIL\n${err.stack || err}`);
  process.exitCode = 1;
});
