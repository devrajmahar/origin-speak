const { app, BrowserWindow, ipcMain, Menu, Tray, protocol, net, screen, session, shell } = require("electron");
const { autoUpdater } = require("electron-updater");
const { spawn } = require("node:child_process");
const { createInterface } = require("node:readline");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

protocol.registerSchemesAsPrivileged([
  {
    scheme: "app",
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
    },
  },
]);

const projectRoot = path.resolve(__dirname, "..");
const devRendererUrl = process.env.ELECTRON_RENDERER_URL;
const allowedCommands = new Set([
  "start_listening", "cancel_listening", "stop_listening", "get_status", "get_audio_level",
  "get_trigger_hotkey", "set_trigger_hotkey", "get_assistant_hotkey", "set_assistant_hotkey",
  "get_language_preferences", "set_language_preferences", "get_vibe_coding_config",
  "set_vibe_coding_config", "list_local_models", "get_transcription_settings",
  "set_transcription_model", "get_transcription_runtime_status", "download_local_model", "type_text",
  "run_system_command", "get_pending_action", "confirm_pending_action", "cancel_pending_action",
  "get_audio_devices", "set_audio_device", "get_history", "clear_history", "hide_assistant",
  "show_dashboard", "get_conversation", "clear_conversation", "new_conversation_session",
  "get_clipboard", "set_clipboard", "get_clipboard_history", "get_integrations",
  "set_integration_enabled", "get_custom_commands", "get_command_templates", "save_custom_command",
  "delete_custom_command", "set_custom_command_enabled", "export_custom_commands",
  "import_custom_commands", "get_autostart_enabled", "set_autostart_enabled", "get_notes",
  "create_note", "update_note", "delete_note", "toggle_note_pin", "create_voice_note",
  "get_snippets", "create_snippet", "update_snippet", "delete_snippet", "get_dictionary_words",
  "add_dictionary_word", "update_dictionary_word", "delete_dictionary_word", "get_errors",
  "get_undismissed_errors", "dismiss_error", "dismiss_all_errors", "learn_correction",
]);

let dashboardWindow;
let assistantWindow;
let tray;
let quitting = false;

class NativeBackend {
  constructor() {
    this.nextId = 1;
    this.pending = new Map();
    this.process = null;
  }

  start() {
    const executable = app.isPackaged
      ? path.join(process.resourcesPath, "backend", process.platform === "win32" ? "listenos-backend.exe" : "listenos-backend")
      : path.join(projectRoot, "backend", "target", "debug", process.platform === "win32" ? "listenos-backend.exe" : "listenos-backend");
    const args = [];

    this.process = spawn(executable, args, {
      cwd: app.isPackaged ? process.resourcesPath : projectRoot,
      env: { ...process.env, RUST_LOG: process.env.RUST_LOG || "info" },
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });

    createInterface({ input: this.process.stdout }).on("line", (line) => this.onMessage(line));
    this.process.stderr.on("data", (chunk) => process.stderr.write(`[native] ${chunk}`));
    this.process.on("error", (error) => {
      this.failAll(error);
      process.stderr.write(`[native] Failed to start: ${error.message}\n`);
      if (!quitting) app.quit();
    });
    this.process.on("exit", (code) => {
      this.failAll(new Error(`Native backend exited with code ${code}`));
      if (!quitting) app.quit();
    });
  }

  onMessage(line) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      process.stderr.write(`[native] Ignoring non-JSON output: ${line}\n`);
      return;
    }

    if (message.event) {
      for (const window of [dashboardWindow, assistantWindow]) {
        if (window && !window.isDestroyed()) {
          window.webContents.send("desktop:event", message.event, message.payload);
        }
      }
      return;
    }

    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    if (message.error) pending.reject(new Error(message.error));
    else pending.resolve(message.result);
  }

  invoke(command, args = {}) {
    if (!this.process?.stdin?.writable) {
      return Promise.reject(new Error("Native backend is not running"));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.process.stdin.write(`${JSON.stringify({ id, command, args })}\n`, (error) => {
        if (error) {
          this.pending.delete(id);
          reject(error);
        }
      });
    });
  }

  failAll(error) {
    for (const { reject } of this.pending.values()) reject(error);
    this.pending.clear();
  }

  stop() {
    if (!this.process) return;
    this.process.stdin.end();
    this.process.kill();
    this.process = null;
  }
}

const backend = new NativeBackend();

function rendererTarget(route = "") {
  if (devRendererUrl) return `${devRendererUrl.replace(/\/$/, "")}${route}`;
  const hash = route ? `#${route.startsWith("/") ? route : `/${route}`}` : "";
  return `app://bundle/index.html${hash}`;
}

function secureWindow(window) {
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (url.startsWith("https://")) void shell.openExternal(url);
    return { action: "deny" };
  });
}

function createDashboardWindow() {
  dashboardWindow = new BrowserWindow({
    title: "ListenOS Dashboard",
    width: 1200,
    height: 800,
    minWidth: 900,
    minHeight: 620,
    center: true,
    frame: false,
    show: false,
    backgroundColor: "#0c0c0c",
    icon: path.join(projectRoot, "backend", "icons", process.platform === "win32" ? "icon.ico" : "icon.png"),
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  secureWindow(dashboardWindow);
  dashboardWindow.loadURL(rendererTarget());
  dashboardWindow.once("ready-to-show", () => dashboardWindow?.show());
  dashboardWindow.on("close", (event) => {
    if (!quitting) {
      event.preventDefault();
      dashboardWindow.hide();
    }
  });
}

function createAssistantWindow() {
  const workArea = screen.getPrimaryDisplay().workArea;
  assistantWindow = new BrowserWindow({
    title: "ListenOS Assistant",
    width: 300,
    height: 70,
    x: Math.round(workArea.x + (workArea.width - 300) / 2),
    y: workArea.y + 24,
    frame: false,
    transparent: true,
    resizable: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    show: false,
    hasShadow: false,
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  secureWindow(assistantWindow);
  assistantWindow.setAlwaysOnTop(true, "floating");
  assistantWindow.loadURL(rendererTarget("/assistant"));
  assistantWindow.once("ready-to-show", () => assistantWindow?.showInactive());
}

function createTray() {
  const icon = path.join(projectRoot, "backend", "icons", process.platform === "win32" ? "icon.ico" : "icon.png");
  tray = new Tray(icon);
  tray.setToolTip("ListenOS");
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: "Open Dashboard", click: () => showDashboard() },
    { type: "separator" },
    { label: "Quit ListenOS", click: () => { quitting = true; app.quit(); } },
  ]));
  tray.on("click", showDashboard);
}

function showDashboard() {
  dashboardWindow?.show();
  dashboardWindow?.focus();
}

function handleDeepLink(url) {
  if (!url?.startsWith("listenos://")) return;
  showDashboard();
  dashboardWindow?.webContents.send("desktop:event", "deep-link", url);
}

function registerIpc() {
  ipcMain.handle("desktop:invoke", async (_event, command, args) => {
    if (!allowedCommands.has(command)) throw new Error(`Blocked native command: ${command}`);

    switch (command) {
      case "hide_assistant":
        assistantWindow?.hide();
        return null;
      case "show_dashboard":
        showDashboard();
        return null;
      case "get_autostart_enabled":
        return app.getLoginItemSettings().openAtLogin;
      case "set_autostart_enabled": {
        const enabled = Boolean(args?.enabled);
        app.setLoginItemSettings({ openAtLogin: enabled, args: ["--minimized"] });
        return app.getLoginItemSettings().openAtLogin;
      }
      default:
        return backend.invoke(command, args);
    }
  });

  ipcMain.handle("window:minimize", (event) => BrowserWindow.fromWebContents(event.sender)?.minimize());
  ipcMain.handle("window:toggle-maximize", (event) => {
    const window = BrowserWindow.fromWebContents(event.sender);
    if (!window) return false;
    if (window.isMaximized()) window.unmaximize();
    else window.maximize();
    return window.isMaximized();
  });
  ipcMain.handle("window:is-maximized", (event) => BrowserWindow.fromWebContents(event.sender)?.isMaximized() ?? false);
  ipcMain.handle("window:close", (event) => BrowserWindow.fromWebContents(event.sender)?.close());
  ipcMain.handle("updates:check", async (_event, silent = true) => {
    if (!app.isPackaged) return { available: false };
    try {
      autoUpdater.autoDownload = false;
      const update = await autoUpdater.checkForUpdates();
      if (!update || update.updateInfo.version === app.getVersion()) return { available: false };
      await autoUpdater.downloadUpdate();
      setImmediate(() => autoUpdater.quitAndInstall(false, true));
      return { available: true, version: update.updateInfo.version };
    } catch (error) {
      if (!silent) throw error;
      return { available: false };
    }
  });
}

const hasSingleInstanceLock = app.requestSingleInstanceLock();
if (!hasSingleInstanceLock) {
  app.quit();
} else {
  app.on("second-instance", (_event, argv) => {
    handleDeepLink(argv.find((arg) => arg.startsWith("listenos://")));
    showDashboard();
  });
  app.on("open-url", (event, url) => {
    event.preventDefault();
    handleDeepLink(url);
  });

  app.whenReady().then(async () => {
    if (!app.isPackaged) app.setAsDefaultProtocolClient("listenos", process.execPath, [projectRoot]);
    else app.setAsDefaultProtocolClient("listenos");

    if (!devRendererUrl) {
      const outputRoot = path.join(app.getAppPath(), "out");
      protocol.handle("app", (request) => {
        const url = new URL(request.url);
        let relativePath = decodeURIComponent(url.pathname).replace(/^\/+/, "");
        if (!relativePath || relativePath.endsWith("/")) relativePath += "index.html";
        const filePath = path.resolve(outputRoot, relativePath);
        if (!filePath.startsWith(`${path.resolve(outputRoot)}${path.sep}`)) {
          return new Response("Forbidden", { status: 403 });
        }
        return net.fetch(pathToFileURL(filePath).toString());
      });
      session.defaultSession.webRequest.onHeadersReceived(
        { urls: ["app://*/*"] },
        (details, callback) => callback({
          responseHeaders: {
            ...details.responseHeaders,
            "Content-Security-Policy": [
              "default-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; script-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
            ],
          },
        }),
      );
    }

    registerIpc();
    backend.start();
    createDashboardWindow();
    createAssistantWindow();
    createTray();

    const deepLink = process.argv.find((arg) => arg.startsWith("listenos://"));
    if (deepLink) handleDeepLink(deepLink);
    if (process.argv.includes("--minimized")) dashboardWindow.hide();
  });
}

app.on("activate", () => {
  if (!dashboardWindow) createDashboardWindow();
  else showDashboard();
});

app.on("before-quit", () => {
  quitting = true;
  backend.stop();
});

app.on("window-all-closed", () => {});
