# Local Wharfedale network in Docker: 4 validators, a full node (rpc0) and the web app.
# Services are in docker/compose.yml; the example and end-to-end scripts run inside node0.

COMPOSE := docker compose -f docker/compose.yml

.PHONY: help run stop clear logs ps example e2e test

help: ## List the targets
	@grep -E '^[a-z0-9]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  make %-8s %s\n", $$1, $$2}'

run: ## Build the images and start the network and the web app (http://localhost:8080)
	$(COMPOSE) up -d --build

stop: ## Stop everything; the chain is kept
	$(COMPOSE) down

clear: ## Stop everything and delete the chain, keys and configs (the next run makes a new genesis)
	$(COMPOSE) down -v

logs: ## Follow the logs of every service
	$(COMPOSE) logs -f

ps: ## Show the services
	$(COMPOSE) ps

example: ## Walkthrough on the running network: transfers, issuance, a Simplicity contract
	$(COMPOSE) exec -T node0 example.sh

e2e: ## End-to-end checks against the running network; fails on the first broken check
	$(COMPOSE) exec -T node0 e2e.sh

test: clear run e2e ## End-to-end checks on a fresh chain
