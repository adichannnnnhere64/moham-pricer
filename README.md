# BugItik Data Update Server

Desktop Tauri app that runs a local Rust HTTP API server for updating mapped
MySQL item rows.

The app does not need a PHP server. Start MySQL, open the desktop app, fill in
the database and mapping fields, then the app starts an embedded API server.

## Local Test Setup

Install dependencies once:

```sh
npm install
```

Start the test MySQL container and apply the test schema:

```sh
make test-up
make test-wait
make test-migrate
```

The Docker MySQL instance is exposed to your host machine on port `3307`.

## Desktop App

Start the Tauri desktop app:

```sh
npm run tauri -- dev
```

Use these values in the app UI for the local Docker database:

| Field | Value |
| --- | --- |
| Host | `127.0.0.1` |
| Port | `3307` |
| Database | `bugitik_test` |
| Username | `bugitik` |
| Password | `bugitik` |
| Bind host | `127.0.0.1` |
| Server port | `8046` |
| API token | `local-dev-token` |
| Table name | `prices` |
| Item ID type | `String` |
| Item ID column | `itemid` |
| Price column | `price` |
| Denomination column | `denomination` |

The API token is your local shared secret. It can be any non-empty value, but the
same value must be sent in the `X-API-Token` header when calling the update API.

Use `127.0.0.1` as the bind host for local testing. Use `0.0.0.0` only if
another device on your network needs to call this machine.

The app default server port is `8045`, but another local container may already
use that port. If the app says the server is not running or cannot bind, use
`8046` in the UI and in the curl commands below.

Click **Start server** after filling the fields.

## Seed A Test Row

The update endpoint only updates existing rows. Add a row before testing:

```sh
docker exec moham-pricer-mysql-1 mysql -ubugitik -pbugitik bugitik_test \
  -e "INSERT INTO prices (itemid, price, denomination) VALUES ('101', 10.00, 'Credits') ON DUPLICATE KEY UPDATE price = VALUES(price), denomination = VALUES(denomination);"
```

Check the row:

```sh
docker exec moham-pricer-mysql-1 mysql -ubugitik -pbugitik bugitik_test \
  -e "SELECT itemid, price, denomination FROM prices WHERE itemid = '101';"
```

`docker compose exec mysql ...` only works when your shell is inside this repo,
where `docker-compose.yml` lives. `docker exec moham-pricer-mysql-1 ...` works
from any directory as long as the container is running.

## API Checks

After the app says the server is running, check health:

```sh
curl http://127.0.0.1:8046/health
```

Update the seeded item:

```sh
curl -X POST http://127.0.0.1:8046/api/items \
  -H "Content-Type: application/json" \
  -H "X-API-Token: local-dev-token" \
  -d '{"itemid":"101","price":"250.00","denomination":"USD"}'
```

Confirm MySQL changed:

```sh
docker exec moham-pricer-mysql-1 mysql -ubugitik -pbugitik bugitik_test \
  -e "SELECT itemid, price, denomination FROM prices WHERE itemid = '101';"
```

Expected result:

```text
itemid  price   denomination
101     250.00  USD
```

## Automated Tests

Run the full local test flow:

```sh
make test
```

This starts MySQL, applies `migrations/test/001_create_prices.sql`, builds the
frontend, and runs the Rust server tests against
`mysql://bugitik:bugitik@127.0.0.1:3307/bugitik_test`.

Stop the container:

```sh
make test-down
```

Remove the container and test database volume:

```sh
make test-clean
```

## API

`POST /api/items` updates one row in the configured table.

Headers:

```text
Content-Type: application/json
X-API-Token: local-dev-token
```

Body:

```json
{
  "itemid": "101",
  "price": "250.00",
  "denomination": "USD"
}
```

Responses:

| Status | Meaning |
| --- | --- |
| `200` | Row updated |
| `400` | Missing fields or non-numeric price |
| `401` | Missing or incorrect API token |
| `404` | No existing row matched `itemid` |
| `500` | Database update failed |

## Troubleshooting

If the desktop app cannot connect to MySQL, make sure the UI port is `3307`.
Docker maps host port `3307` to MySQL's internal port `3306`.

If the update call returns `404`, seed the row first. The API runs an `UPDATE`,
not an insert.

If the update call returns `401`, the `X-API-Token` header does not exactly
match the token saved in the app UI.

If port `8045` is already in use, change the server port in the app to `8046`
and use that same port in curl.
