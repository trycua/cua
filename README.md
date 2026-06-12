# Snorkel TerminalBench+ Edition 2 Data Sample

**Version:** February 2026 (v2.0)
**Contact:** zach.kleinbaum@snorkel.ai, armin.parchami@snorkel.ai

## Disclaimer

Disclaimer: These materials and data samples are provided solely for evaluation and discussion purposes and do not constitute a delivery under any existing agreement between Snorkel AI, Inc. and the recipient. All rights, title, and interest in these materials and data samples remain the exclusive property of Snorkel AI, Inc. The recipient may not copy, distribute, or use these materials or data samples for any commercial or competitive purpose without Snorkel AI Inc.'s prior written consent or a separately executed license agreement. The materials and data samples are provided "AS IS" and without warranties of any kind. By accessing or using the materials or data samples, the recipient agrees to these terms.

## Overview

Terminal-Bench is a benchmark designed to evaluate large language model (LLM) agents in realistic, interactive computing environments, where they must execute complex, multi-step workflows using command-line tools and programming languages. It covers a diverse range of domains including software engineering, data science, machine learning, DevOps, database management, security, and scientific computing.

This dataset represents samples from **Snorkel TerminalBench+ Edition 2**, the second entry in the Snorkel Agentic Coding Data Series. Snorkel is a leading contributor to Terminal-Bench 2.0. More details on our contributions: https://snorkel.ai/blog/evaluating-coding-agent-capabilities-with-terminal-bench-snorkels-role-in-building-the-next-generation-benchmark/

## What's New in Edition 2

Edition 1 (released December 2025) provided 6,000 expert-authored tasks testing individual competencies in isolation. Edition 2 moves beyond single-skill evaluation to composite, multi-step tasks that mirror real engineering workflows — requiring agents to chain competencies across milestones in larger, more complex environments. Together, the two editions form a sequenced training curriculum. Edition 2 is one of several editions planned for release throughout 2026. The Terminal-Bench+ series will deliver 15,000+ expert-authored tasks.

### Key Innovations

**1. Milestone-Based Evaluation**

Edition 2 introduces tasks divided into 2–5 independently verifiable milestones that the agent must complete in sequence. Each milestone has its own test verifiers and rubrics, yielding 3–5x more training signal per task. This enables diagnostic precision — instead of knowing the agent failed, you know it failed at milestone 3 after succeeding at milestones 1 and 2.

**2. Real-World Context Layers**

Edition 1 defines what type of problem an agent must solve. Edition 2 adds a second axis defining the conditions under which the agent must work. These cut across all task types:

| Context Layer | What It Tests |
|---|---|
| Long Context | Reasoning over 50,000+ token documents that cannot be parsed programmatically |
| API Integration | Building, debugging, and interacting with APIs where source code is in the environment |
| DB Interaction | Solving problems through direct database querying (SQL, NoSQL, vector databases) |
| UI Building | Creating and modifying UIs verified via Playwright and Vitest |
| Tool-Specific | Working with niche tools that have complex SDKs but limited training data representation |

**3. Codebase Scale Variation**

Edition 1 tasks operate in contained environments. Edition 2 systematically varies environment complexity with labeled codebase sizes:

| Scale | Description |
|---|---|
| Large | 200+ files, production-scale with documentation and test suites |
| Small | 20+ files, aligned to smaller services and projects |
| Minimal | Little to no codebase context — pure terminal tasks |

### Edition 1 vs. Edition 2 at a Glance

| Dimension | Edition 1 | Edition 2 |
|---|---|---|
| Task scope | Atomic (1 competency per task) | Composite (2–5 competencies chained across milestones) |
| Evaluation | Pass/fail + rubrics | Milestone-level verifiers + rubrics per milestone |
| Reward signal | Binary (1 signal per task) | Dense trajectory (per milestone + per rubric) |
| Real-world context | — | Long Context, API Integration, DB Interaction, UI Building, Tool-Specific |
| Codebase scale | Not differentiated | Large / Small / Minimal, labeled in metadata |
| Languages | Primarily Python | Python ≤50%, 5+ languages at ≥5%, multi-language tasks |
| Difficulty calibration | GPT-5, Claude Sonnet 4.5 | GPT-5.2, Claude Opus 4.6 |

## Task Type Taxonomy

The tasks in this dataset span multiple categories that reflect real-world technical work:

- **Software Engineering:** Tasks focused on developing or testing features and algorithms, fixing bugs and improving/optimizing existing features, implementing tests, or maintaining software projects.

- **Machine Learning / Model Training / Inference:** Tasks requiring training, fine-tuning, running inference, or evaluating machine learning models, including dependency setup, running training loops, and managing data pipelines for ML tasks.

- **System/Environment Setup & Configuration:** Tasks involving OS-level configuration, user management, package management, processes, or installing, configuring, and bringing up services, networks, and environments.

- **Build / Compilation / Dependency Management:** Compile code, manage dependencies, and build components.

- **Data / File Processing / ETL / Scripting:** Tasks that transform, parse, filter, and aggregate datasets or files and directories to generate derived output.

- **Interactive / Simulation Tasks / Games:** Tasks centered on game-like or simulated environments, interactive puzzles, or simulation games that run in the terminal.

- **Debugging / Repair / Fixing Tasks:** Tasks that require identifying, diagnosing, and fixing errors in scripts, codebases, or system configurations.

- **Security / Cryptography / Vulnerability Demonstration:** Tasks related to cryptography, authentication, permissions, penetration-style tests, exploits, validating vulnerabilities, reverse engineering, or security configuration.

- **Scientific Computing:** Tasks using scientific libraries or workflows, such as numerical computation, simulations, or domain-specific research code.

## Data Sample

This sample contains **50 tasks** from the Snorkel TerminalBench+ offering spanning software engineering, scientific computing, debugging, data processing, security, machine learning, system administration, system setup & configuration, and games categories.

### Difficulty Tiers

Each task is categorized based on model performance using GPT-5.2 + Terminus 2 agent and Claude Opus 4.6 + Terminus 2 agent. Difficulty tiers are defined as follows:

| Tier | Definition |
|---|---|
| **Frontier** | Accuracy <= 20% on best model |
| **Advanced Plus** | Accuracy <= 20% on worst model (exclusive of Frontier) |
| **Advanced** | 20% < Accuracy <= 60% on worst model |
| **Core** | 60% < Accuracy <= 80% on worst model |

The difficulty breakdown of these 50 sample tasks is:

- 6 Frontier
- 24 Advanced Plus
- 19 Advanced
- 1 Core

## Task Structure

Each task directory contains the following required files:

- **`task.toml`** - Contains the task metadata including difficulty level, category, timeout settings, and other configuration parameters

- **`instruction.md`** - Detailed task instructions in Markdown format that describe what needs to be accomplished

- **`environment/Dockerfile`** - Defines the Docker environment and dependencies required to run the task

- **`solution/solve.sh`** - Reference solution created by a human expert demonstrating how to complete the task correctly

- **`tests/test.sh`** - Shell script that executes the test suite to verify task completion

- **`tests/test_outputs.py`** - Deterministic Python tests that check whether the task was solved correctly based on the final state of the environment (or `tests/test_outputs.spec.ts` for Playwright-verified UI tasks)

- **`rubrics.txt`** - Evaluation criteria for LLM-as-judge scoring of the agent's trace, covering both process and outcome (or multiple `rubrics_x.txt` files for milestone tasks)

Note: Tasks may also include additional files such as `milestones.md`, per-milestone test files (`tests/test_m*.py`), per-milestone solution scripts (`solution/solve*.sh`), and other data or configuration files specific to the task requirements.

## Task Summaries

This sample includes the following tasks:

### Software Engineering (18 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `api-incident-escalation-repair-v5` | Advanced | Repair a broken Flask on-call service — fix incident listing, risk scoring, and escalation plan generation across 3 milestones. |
| `cartpole-ppo-reward-shaping-grid-search` | Advanced Plus | Fix a broken PPO reward-grid search script for CartPole-v1 producing JSON/CSV artifacts with Pareto flags. |
| `ci-pipeline-monitor` | Advanced | Fix a CI pipeline monitor extracting build logs and computing health metrics across 4 milestones. |
| `data-migration-pipeline` | Advanced Plus | Fix a warehouse migration pipeline pulling shipment CSVs into SQLite with transformation and reporting across 4 milestones. |
| `doc-analysis-api` | Advanced Plus | Extend a Flask app with a document analysis API supporting full-text search and TF-IDF processing across 3 milestones. |
| `expense-report` | Advanced | Fix an expense report processor that reads JSON files, converts currencies, and generates reports across 4 milestones. |
| `fix-json-patch` | Advanced Plus | Fix a broken RFC 6902 JSON Patch and RFC 6901 JSON Pointer implementation (small codebase). |
| `github-actions-yaml-fix_20260307_134248` | Advanced Plus | Fix multiple errors in a GitHub Actions CI YAML preventing the pipeline from running (small codebase). |
| `go-microservice-user-api` | Advanced Plus | Build a Go microservice with user CRUD operations on SQLite, fixing a broken database connection skeleton (small codebase). |
| `gym-gridworld-multi-objective-pareto-cli` | Core | Fix a broken multi-objective Gymnasium grid-world CLI producing Pareto-optimal configs with JSON/CSV reports. |
| `headless-git-release-surgery` | Advanced Plus | Fix a git repository left in a broken release state after partial cherry-pick/revert work and complete the 2.4.1 release. |
| `jwt_auth_service_evolution_task` | Frontier | Build a Flask authentication microservice evolving through JWT auth, OAuth2 server, and social login across 3 milestones. |
| `kuhn-poker-cfr-plus-selfplay-cli` | Advanced | Fix a broken CFR+ self-play implementation for Kuhn Poker producing correct exploitability and policy JSON. |
| `kuhn-poker-openspiel-cfr-plus-policy-export` | Frontier | Fix an OpenSpiel Kuhn Poker CFR+ trainer and export average policy meeting strict exploitability thresholds. |
| `multi-agent-pursuit-evasion-selfplay-ppo` | Frontier | Fix a broken multi-agent pursuit-evasion RL prototype with PPO self-play training. |
| `redis-caching-express-js` | Advanced Plus | Review and fix a Redis-based caching layer added to an Express.js API. |
| `text-analysis-visualization-react` | Advanced Plus | Build a CLI text analysis workflow for large documents and a React UI for visualizing generated insights. |
| `unicode-filename-normalizer-cli` | Advanced | Fix a broken Python CLI that normalizes filesystem names for a migration script. |

### Scientific Computing (10 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `battery-dispatch-opt` | Advanced Plus | Write a dispatch controller for a grid-scale battery energy storage system to optimize buy/sell decisions. |
| `chem-kinetics-sim` | Frontier | Build a CHEMKIN-format kinetics simulator across 4 milestones: thermodynamic parsing, rate constants, ODE integration, and sensitivity analysis. |
| `dc-workload-opt` | Advanced | Write a resource controller for a data center with an indoor battery storage system to optimize workload scheduling (small codebase). |
| `geochem-reactive-transport` | Advanced Plus | Build a geochemical reactive transport simulator with thermodynamics, equilibrium, and reactive flow across 4 milestones. |
| `multiref-qchem-engine` | Advanced Plus | Build a multi-configurational quantum chemistry engine implementing CI, CASSCF, NEVPT2, and FMO2 methods across 4 milestones. |
| `phase-coupling-inference` | Advanced Plus | Infer coupling coefficients from recordings of 30 coupled phase oscillators over 5000 timesteps. |
| `pumped-hydro-dispatch` | Advanced Plus | Write a dispatch controller for a pumped hydro storage facility to optimize grid power usage and water pumping. |
| `qchem-output-parser` | Advanced Plus | Build a Python library parsing output files from 5 quantum chemistry programs with UV-Vis simulation and thermochemistry across 4 milestones. |
| `sparse-preconditioner-synthesis` | Advanced | Fix a sparse linear system solver driver that loads problem specs and applies preconditioning (small codebase). |
| `xc-functional-library-v2` | Advanced | Build a Python exchange-correlation functional library with LDA, GGA, and hybrid functionals across 3 milestones. |

### Debugging / Repair (6 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `c-react-stats-task` | Advanced Plus | Fix a three-layer data visualization dashboard with a C statistical engine, Express API, and React frontend across 3 milestones (small codebase). |
| `circuit-debugger` | Advanced Plus | Debug a logic circuit server with 8 two-input gates by probing inputs/outputs to identify the faulty gate. |
| `etl-csv-image-tool` | Advanced Plus | Fix an Express/SQLite ETL pipeline for CSV ingestion, statistical analysis, and image processing across 3 milestones. |
| `flaky-test-suite-debugging` | Advanced | Diagnose and fix a flaky Python test suite whose failures depend on execution order and shared state. |
| `kotlin-mlflow-task` | Advanced | Debug a Kotlin linear regression project with MLflow tracking — fix build, model training, and inference API across 3 milestones. |
| `python-docker-c-extension-fix` | Advanced Plus | Fix a Python application with a C extension module that builds successfully but crashes at runtime (small codebase). |

### Data Processing (5 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `csv-profiler-cli-updated` | Advanced Plus | Build a CLI tool to profile CSV datasets with statistical summaries and visualization. |
| `data-contract-compliance-enforcer-v5` | Advanced Plus | Enforce data contract compliance in a warehouse snapshot pipeline with schema auditing and quality checks across 3 milestones. |
| `grand-slam-upset-index-v4` | Advanced | Build a ranked Grand Slam upset index from 1990–2024 match CSV data, grouped by surface and decade, with JSON output. |
| `panel-position-extractor-v8` | Advanced Plus | Extract per-speaker positions and agreements from a ~55k-word panel transcript and produce structured JSON output. |
| `sql-agent` | Frontier | Explore an unknown SQLite database, decode encoded values via a codec registry, and reconstruct a secret token through foreign key chain analysis. |

### Security (5 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `issue-x509-server-certificates` | Advanced | Build a small PKI with OpenSSL: root CA, intermediate CA, and multiple X.509 server certificates with appropriate extensions. |
| `linux-elf-malware-correlator` | Advanced | Analyze a Linux ELF malware sample — unpack if UPX-packed, run static analysis, and correlate with threat intel across 4 milestones. |
| `linux-persistence-hunter-remediator-cli` | Advanced Plus | Build a CLI tool to hunt stealthy Linux persistence mechanisms and generate a remediation report across 4 milestones. |
| `opaque-wire-decode` | Frontier | Reverse-engineer an undocumented industrial monitoring protocol from a binary capture and decode 397 length-prefixed messages. |
| `secure-flask-api` | Advanced Plus | Audit and fix security vulnerabilities in a Flask REST API handling user registration, JWT authentication, and profile management (small codebase). |

### Machine Learning (2 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `debug-ml-training-pipeline` | Advanced | Fix a broken binary classifier training pipeline with a custom weighted focal loss on an imbalanced dataset (small codebase). |
| `parallel-rollout-prioritized-replay-buffer` | Advanced | Fix a broken RL utility implementing parallel rollouts with a prioritized experience replay buffer. |

### System Administration (1 task)

| Task | Difficulty | Description |
|------|------------|-------------|
| `cis-level1-linux-compliance-cli` | Advanced | Build a CIS Level 1 Linux compliance checker that evaluates system controls and emits a remediation script across 4 milestones. |

### System Setup & Configuration (1 task)

| Task | Difficulty | Description |
|------|------------|-------------|
| `terraform-gcp-env` | Advanced | Create production-ready Terraform config for a GCP environment with VPC, Cloud SQL, GCS, and Redis with remote state. |

### Games (2 tasks)

| Task | Difficulty | Description |
|------|------------|-------------|
| `blind-auction` | Advanced | Play a blind auction game via HTTP API — maximize winnings across 8 rounds by bidding strategically on items. |
| `process-scheduler` | Advanced | Interact with a CPU job scheduler server to produce optimal process schedules for 5 evaluation seeds. |

## Milestone Tasks

The following tasks use a milestone-based structure where the agent must complete sequential stages of increasing complexity:

- **`api-incident-escalation-repair-v5`** (3 milestones): Repair incident listing/filtering/pagination; repair risk scoring; generate escalation plan with checksum.

- **`c-react-stats-task`** (3 milestones): Build System & Server Startup; Statistical Computation; Visualization & Integration.

- **`chem-kinetics-sim`** (4 milestones): Parse CHEMKIN thermodynamic properties; compute rate constants with Troe falloff; integrate reactor ODEs; perform sensitivity and rate-of-production analysis.

- **`ci-pipeline-monitor`** (4 milestones): Fix data parsing and aggregation; fix flaky test detection and build health; fix stage ranking and report summary; fix trend analysis.

- **`cis-level1-linux-compliance-cli`** (4 milestones): Load benchmark JSON and implement control parser; scan system settings and evaluate controls; produce compliance report JSON; emit POSIX-compliant remediation script.

- **`data-contract-compliance-enforcer-v5`** (3 milestones): Build schema auditor; build quality and freshness checks; build compliance summarizer.

- **`data-migration-pipeline`** (4 milestones): Fix data loading; fix processing and reporting; fix data enrichment; fix audit trail.

- **`doc-analysis-api`** (3 milestones): Set up document fetching API endpoints; implement full-document analysis with TF-IDF, bigrams, and similarity; build results listing and aggregate statistics.

- **`etl-csv-image-tool`** (3 milestones): CSV Ingestion Pipeline; Analysis Engine; Advanced Analytics and Dashboard.

- **`expense-report`** (4 milestones): Fix data loading and parsing; correct reporting scope and limit checks; fix approver dict mutation and categorizer sorting; resolve numeric tax edge-cases and per-diem dates.

- **`geochem-reactive-transport`** (4 milestones): Thermodynamic database parser and activity coefficient models; Gibbs energy minimization with Newton-Raphson; 1D reactive transport with MUSCL advection-dispersion; mineral saturation indices and benchmark validation.

- **`jwt_auth_service_evolution_task`** (3 milestones): JWT Authentication Foundation; OAuth2 Authorization Server; Social Login Integration.

- **`kotlin-mlflow-task`** (3 milestones): Fix Kotlin build environment; fix model training with correct normalization and MLflow integration; fix inference API route paths and prediction denormalization.

- **`linux-elf-malware-correlator`** (4 milestones): Detect and unpack UPX sample; produce static imports and strings; run strace and capture syscalls; correlate and output structured report JSON.

- **`linux-persistence-hunter-remediator-cli`** (4 milestones): Hunter CLI with inventory mode; classification against allowlist; safe remediation with atomic report; validate mode confirming persistence state.

- **`multiref-qchem-engine`** (4 milestones): Full CI with Davidson diagonalization; CASSCF orbital optimization; NEVPT2 perturbation theory; restricted Hartree-Fock and FMO2.

- **`qchem-output-parser`** (4 milestones): Gaussian/ORCA parsers with core unit conversion; NWChem/GAMESS/Psi4 parsers with auto-detection; UV-Vis simulation and thermochemistry; JSON serialization and CLI.

- **`xc-functional-library-v2`** (3 milestones): LDA functionals with analytical derivatives; GGA functionals with gradient-dependent derivatives; hybrid functionals with expression parser and finite-difference verification.
