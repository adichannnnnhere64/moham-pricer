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

type ApiRequestLog = {
  id: number;
  timestampMs: number;
  remoteAddr: string | null;
  method: string;
  path: string;
  status: number;
  durationMs: number;
  itemid: string | null;
  message: string;
};

type RequestMetrics = {
  total: number;
  ok: number;
  errors: number;
  avgDurationMs: number;
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

const defaultMetrics: RequestMetrics = {
  total: 0,
  ok: 0,
  errors: 0,
  avgDurationMs: 0,
};

function App() {
  const [config, setConfig] = useState<ServerConfig>(defaultConfig);
  const [status, setStatus] = useState<ServerStatus>(defaultStatus);
  const [machineIp, setMachineIp] = useState<string | null>(null);
  const [history, setHistory] = useState<ApiRequestLog[]>([]);
  const [metrics, setMetrics] = useState<RequestMetrics>(defaultMetrics);
  const [message, setMessage] = useState("Load settings to begin.");
  const [busy, setBusy] = useState(false);

  const endpoint = useMemo(() => {
    const bindHost = config.bindHost.trim();
    const host =
      bindHost === "0.0.0.0" || bindHost === ""
        ? machineIp ?? "localhost"
        : bindHost;
    return `http://${host}:${config.serverPort || 0}${updateEndpointPath}`;
  }, [config.bindHost, config.serverPort, machineIp]);

  useEffect(() => {
    async function load() {
      try {
        const [
          saved,
          currentStatus,
          currentMachineIp,
          currentHistory,
          currentMetrics,
        ] = await Promise.all([
          invoke<ServerConfig>("load_settings"),
          invoke<ServerStatus>("server_status"),
          invoke<string | null>("machine_ip"),
          invoke<ApiRequestLog[]>("request_history"),
          invoke<RequestMetrics>("request_metrics"),
        ]);
        setConfig({ ...defaultConfig, ...saved });
        setStatus(currentStatus);
        setMachineIp(currentMachineIp);
        setHistory(currentHistory);
        setMetrics(currentMetrics);
        setMessage("Settings loaded.");
      } catch (error) {
        setMessage(formatError(error));
      }
    }

    load();
  }, []);

  useEffect(() => {
    let mounted = true;

    async function refreshTelemetry() {
      try {
        const [currentStatus, currentHistory, currentMetrics] = await Promise.all([
          invoke<ServerStatus>("server_status"),
          invoke<ApiRequestLog[]>("request_history"),
          invoke<RequestMetrics>("request_metrics"),
        ]);
        if (!mounted) {
          return;
        }
        setStatus(currentStatus);
        setHistory(currentHistory);
        setMetrics(currentMetrics);
      } catch (error) {
        if (mounted) {
          setMessage(formatError(error));
        }
      }
    }

    const interval = window.setInterval(refreshTelemetry, 1000);
    return () => {
      mounted = false;
      window.clearInterval(interval);
    };
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

  async function clearHistory() {
    try {
      await invoke("clear_request_history");
      setHistory([]);
      setMetrics(defaultMetrics);
      setMessage("Request history cleared.");
    } catch (error) {
      setMessage(formatError(error));
    }
  }

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Pricer API</p>
          <h1>Connection console</h1>
          <p className="lede">
            Watch requests land, tune the MySQL mapper, and keep the local API ready for traffic.
          </p>
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

      <section className="metrics" aria-label="Request metrics">
        <article>
          <span>Total hits</span>
          <strong>{metrics.total}</strong>
        </article>
        <article>
          <span>Successful</span>
          <strong>{metrics.ok}</strong>
        </article>
        <article>
          <span>Errors</span>
          <strong>{metrics.errors}</strong>
        </article>
        <article>
          <span>Avg time</span>
          <strong>{metrics.avgDurationMs} ms</strong>
        </article>
      </section>

      <section className="traffic">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Live traffic</p>
            <h2>Connection history</h2>
          </div>
          <button type="button" className="secondary compact" onClick={clearHistory}>
            Clear history
          </button>
        </div>
        {history.length === 0 ? (
          <div className="empty-state">
            Start the server and incoming API hits will appear here.
          </div>
        ) : (
          <div className="history-list" role="list">
            {history.map((entry) => (
              <article className="history-row" key={entry.id} role="listitem">
                <div className="history-main">
                  <span className={entry.status < 400 ? "code ok" : "code error"}>
                    {entry.status}
                  </span>
                  <div>
                    <strong>
                      {entry.method} {entry.path}
                    </strong>
                    <p>
                      {entry.message}
                      {entry.itemid ? ` - item ${entry.itemid}` : ""}
                    </p>
                  </div>
                </div>
                <div className="history-meta">
                  <span>{entry.durationMs} ms</span>
                  <span>{entry.remoteAddr ?? "unknown client"}</span>
                  <time dateTime={new Date(Number(entry.timestampMs)).toISOString()}>
                    {formatTimestamp(entry.timestampMs)}
                  </time>
                </div>
              </article>
            ))}
          </div>
        )}
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
  -d '{"itemid":"101","price":"250.00"}'`}</pre>
        {status.lastError ? <p className="error">{status.lastError}</p> : null}
      </section>
    </main>
  );
}

function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function formatTimestamp(timestampMs: number) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(Number(timestampMs)));
}

export default App;
