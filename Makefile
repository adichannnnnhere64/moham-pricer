COMPOSE := docker compose
MYSQL_SERVICE := mysql
MYSQL_ROOT_PASSWORD := bugitik_root
MYSQL_DATABASE := bugitik_test

.PHONY: test test-up test-wait test-migrate test-down test-clean

test: test-up test-wait test-migrate
	npm run build
	RUSTC_WRAPPER=$(CURDIR)/scripts/rustc-no-check-cfg.sh BUGITIK_TEST_DATABASE_URL=mysql://bugitik:bugitik@127.0.0.1:3307/bugitik_test cargo test --manifest-path src-tauri/Cargo.toml --no-default-features server::tests:: -- --test-threads=1

test-up:
	$(COMPOSE) up -d $(MYSQL_SERVICE)

test-wait:
	@printf "Waiting for MySQL"
	@for i in $$(seq 1 60); do \
		if $(COMPOSE) exec -T $(MYSQL_SERVICE) mysqladmin ping -h 127.0.0.1 -ubugitik -pbugitik --silent >/dev/null 2>&1; then \
			printf "\nMySQL is ready\n"; \
			exit 0; \
		fi; \
		printf "."; \
		sleep 1; \
	done; \
	printf "\nMySQL did not become ready in time\n"; \
	exit 1

test-migrate:
	$(COMPOSE) exec -T $(MYSQL_SERVICE) mysql -uroot -p$(MYSQL_ROOT_PASSWORD) $(MYSQL_DATABASE) < migrations/test/001_create_prices.sql

test-down:
	$(COMPOSE) down

test-clean:
	$(COMPOSE) down -v --remove-orphans
