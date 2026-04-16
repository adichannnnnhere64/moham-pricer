import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type ItemIdType = "string" | "integer";

type ServerConfig = {
  mysqlHost: string;
  mysqlPort: number;
  mysqlDatabase: string;
  mysqlUsername: string;
  mysqlPassword: string;
  bindHost: string;
  serverPort: number;
  apiToken: string;
  tableName: string;
  itemIdColumn: string;
  priceColumn: string;
  denominationColumn: string;
  itemIdType: ItemIdType;
};

type ServerStatus = {
  running: boolean;
  bindAddress: string | null;
  lastError: string | null;
};

const updateEndpointPath = "/api/items";

const defaultConfig: ServerConfig = {
  mysqlHost: "127.0.0.1",
  mysqlPort: 3306,
  mysqlDatabase: "",
  mysqlUsername: "",
  mysqlPassword: "",
  bindHost: "0.0.0.0",
  serverPort: 8045,
  apiToken: "",
  tableName: "",
  itemIdColumn: "itemid",
  priceColumn: "price",
  denominationColumn: "denomination",
  itemIdType: "string",
};

const defaultStatus: ServerStatus = {
  running: false,
  bindAddress: null,
  lastError: null,
};

function App() {
  const [config, setConfig] = useState<ServerConfig>(defaultConfig);
  const [status, setStatus] = useState<ServerStatus>(defaultStatus);
  const [message, setMessage] = useState("Load settings to begin.");
  const [busy, setBusy] = useState(false);

  const endpoint = useMemo(() => {
    const host =
      config.bindHost === "0.0.0.0" || config.bindHost.trim() === ""
        ? "[machine-ip]"
        : config.bindHost;
    return `http://${host}:${config.serverPort || 0}${updateEndpointPath}`;
  }, [config.bindHost, config.serverPort]);

  useEffect(() => {
    async function load() {
      try {
        const saved = await invoke<ServerConfig>("load_settings");
        const currentStatus = await invoke<ServerStatus>("server_status");
        setConfig({ ...defaultConfig, ...saved });
        setStatus(currentStatus);
        setMessage("Settings loaded.");
      } catch (error) {
        setMessage(formatError(error));
      }
    }

    load();
  }, []);

  function updateField<K extends keyof ServerConfig>(
    key: K,
    value: ServerConfig[K],
  ) {
    setConfig((current) => ({ ...current, [key]: value }));
  }

  async function saveSettings() {
    setBusy(true);
    try {
      await invoke("save_settings", { config });
      setMessage("Settings saved.");
    } catch (error) {
      setMessage(formatError(error));
    } finally {
      setBusy(false);
    }
  }

  async function startServer() {
    setBusy(true);
    try {
      const nextStatus = await invoke<ServerStatus>("start_api_server", {
        config,
      });
      setStatus(nextStatus);
      setMessage(`Server running at ${nextStatus.bindAddress}.`);
    } catch (error) {
      setStatus((current) => ({
        ...current,
        running: false,
        lastError: formatError(error),
      }));
      setMessage(formatError(error));
    } finally {
      setBusy(false);
    }
  }

  async function stopServer() {
    setBusy(true);
    try {
      const nextStatus = await invoke<ServerStatus>("stop_api_server");
      setStatus(nextStatus);
      setMessage("Server stopped.");
    } catch (error) {
      setMessage(formatError(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="shell">
      <header className="topbar">
        <img src="/tauri.svg" alt="" className="app-mark" />
        <div>
          <p className="eyebrow">BugItik local API</p>
          <h1>Data update server</h1>
        </div>
        <div className={status.running ? "status running" : "status stopped"}>
          {status.running ? "Running" : "Stopped"}
        </div>
      </header>

      <section className="notice">
        <div>
          <strong>Endpoint</strong>
          <code>{endpoint}</code>
        </div>
        <p>{message}</p>
      </section>

      <form
        className="settings"
        onSubmit={(event) => {
          event.preventDefault();
          startServer();
        }}
      >
        <section className="panel">
          <h2>MySQL connection</h2>
          <div className="grid two">
            <label>
              Host
              <input
                value={config.mysqlHost}
                onChange={(event) =>
                  updateField("mysqlHost", event.currentTarget.value)
                }
                placeholder="127.0.0.1"
              />
            </label>
            <label>
              Port
              <input
                type="number"
                min="1"
                max="65535"
                value={config.mysqlPort}
                onChange={(event) =>
                  updateField("mysqlPort", Number(event.currentTarget.value))
                }
              />
            </label>
            <label>
              Database
              <input
                value={config.mysqlDatabase}
                onChange={(event) =>
                  updateField("mysqlDatabase", event.currentTarget.value)
                }
              />
            </label>
            <label>
              Username
              <input
                value={config.mysqlUsername}
                onChange={(event) =>
                  updateField("mysqlUsername", event.currentTarget.value)
                }
              />
            </label>
            <label className="span-two">
              Password
              <input
                type="password"
                value={config.mysqlPassword}
                onChange={(event) =>
                  updateField("mysqlPassword", event.currentTarget.value)
                }
              />
            </label>
          </div>
        </section>

        <section className="panel">
          <h2>HTTP server</h2>
          <div className="grid two">
            <label>
              Bind host
              <input
                value={config.bindHost}
                onChange={(event) =>
                  updateField("bindHost", event.currentTarget.value)
                }
                placeholder="0.0.0.0"
              />
            </label>
            <label>
              Server port
              <input
                type="number"
                min="1"
                max="65535"
                value={config.serverPort}
                onChange={(event) =>
                  updateField("serverPort", Number(event.currentTarget.value))
                }
              />
            </label>
            <label className="span-two">
              API token
              <input
                type="password"
                value={config.apiToken}
                onChange={(event) =>
                  updateField("apiToken", event.currentTarget.value)
                }
                placeholder="Required in X-API-Token"
              />
            </label>
          </div>
        </section>

        <section className="panel">
          <h2>Database mapping</h2>
          <div className="grid two">
            <label>
              Table name
              <input
                value={config.tableName}
                onChange={(event) =>
                  updateField("tableName", event.currentTarget.value)
                }
                placeholder="items"
              />
            </label>
            <label>
              Item ID type
              <select
                value={config.itemIdType}
                onChange={(event) =>
                  updateField("itemIdType", event.currentTarget.value as ItemIdType)
                }
              >
                <option value="string">String</option>
                <option value="integer">Integer</option>
              </select>
            </label>
            <label>
              Item ID column
              <input
                value={config.itemIdColumn}
                onChange={(event) =>
                  updateField("itemIdColumn", event.currentTarget.value)
                }
              />
            </label>
            <label>
              Price column
              <input
                value={config.priceColumn}
                onChange={(event) =>
                  updateField("priceColumn", event.currentTarget.value)
                }
              />
            </label>
            <label className="span-two">
              Denomination column
              <input
                value={config.denominationColumn}
                onChange={(event) =>
                  updateField("denominationColumn", event.currentTarget.value)
                }
              />
            </label>
          </div>
        </section>

        <section className="actions">
          <button type="button" className="secondary" onClick={saveSettings} disabled={busy}>
            Save settings
          </button>
          <button type="submit" disabled={busy}>
            {status.running ? "Restart server" : "Start server"}
          </button>
          <button
            type="button"
            className="danger"
            onClick={stopServer}
            disabled={busy || !status.running}
          >
            Stop server
          </button>
        </section>
      </form>

      <section className="sample">
        <h2>Client request</h2>
        <pre>{`curl -X POST ${endpoint} \\
  -H "Content-Type: application/json" \\
  -H "X-API-Token: ${config.apiToken ? "<token>" : "your-token"}" \\
  -d '{"itemid":"101","price":"250.00","denomination":"Credits"}'`}</pre>
        {status.lastError ? <p className="error">{status.lastError}</p> : null}
      </section>
    </main>
  );
}

function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export default App;
