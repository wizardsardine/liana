# SonarQube Local Setup for Coincube

A quick guide to set up SonarQube locally for the Coincube Rust project.

## Prerequisites

- Docker and Docker Compose installed
- `sonar-scanner` CLI installed (`brew install sonar-scanner` on macOS)
- Access to GitHub releases for downloading the Rust plugin

## Step 1: Download the Rust Plugin

The Community Rust plugin must be manually downloaded. **Use v0.1.4** — newer versions require SonarQube Plugin API 10.11.0.2468 which is not available in stable SonarQube Community images.

```bash
mkdir -p ~/sonarqube/plugins
curl -L -o ~/sonarqube/plugins/community-rust-plugin-0.1.4.jar \
  https://github.com/elegoff/sonar-rust/releases/download/v0.1.4/community-rust-plugin-0.1.4.jar
```

## Step 2: Configure Docker Compose

Create or update `docker-compose.yml` in the project root:

```yaml
version: "3.8"

services:
  sonarqube:
    image: sonarqube:10.6-community
    container_name: sonarqube
    ports:
      - "9000:9000"
    environment:
      SONAR_ES_BOOTSTRAP_CHECKS_DISABLE: "true"
    volumes:
      - sonarqube_data:/opt/sonarqube/data
      - ~/sonarqube/plugins:/opt/sonarqube/extensions/plugins

volumes:
  sonarqube_data:
```

**Note:** `sonarqube:10.6-community` provides Plugin API 10.7.0.2191 which is compatible with Rust plugin v0.1.4.

## Step 3: Start SonarQube

```bash
# Stop any existing containers and clean data if needed
docker compose down
docker volume rm coincube_sonarqube_data 2>/dev/null || true

# Start SonarQube
docker compose up -d

# Wait for startup (watch for "SonarQube is up" in logs)
docker logs -f sonarqube
```

## Step 4: Initial Setup in SonarQube UI

1. Open http://localhost:9000
2. Log in with default credentials: `admin` / `admin`
3. Change password when prompted

### Create the Project

1. Go to **Administration → Projects → Management**
2. Click **Create Project → Local Project**
3. Set **Project Key**: `coincubetech_coincube`
4. Set **Display Name**: `Coincube`

### Generate Scanner Token

1. Go to **User → My Account → Security → Generate Tokens**
2. Name: `local-scan`
3. Type: **Global Analysis Token**
4. Copy the generated token

## Step 5: Configure Project Properties

Create/update `sonar-project.properties` in the project root:

```properties
sonar.projectKey=coincubetech_coincube
sonar.projectName=Coincube
sonar.sources=./coincube-core/src,./coincubed/src,./coincube-gui/src,./coincube-ui/src
sonar.host.url=http://localhost:9000
sonar.token=YOUR_GENERATED_TOKEN_HERE

# Rust language settings
sonar.language=rust

# Coverage report location (optional)
# sonar.rust.coverage.reportPaths=coverage.xml

# Ignore rules
sonar.issue.ignore.multicriteria=e1,e2,e3

# Ignore string duplication warnings in Rust test files
sonar.issue.ignore.multicriteria.e1.ruleKey=rust:S1192
sonar.issue.ignore.multicriteria.e1.resourceKey=**/tests/**/*.rs

# Ignore function naming conventions for test modules
sonar.issue.ignore.multicriteria.e2.ruleKey=rust:S100
sonar.issue.ignore.multicriteria.e2.resourceKey=**/*_test.rs

# Ignore cognitive complexity in UI state machines (complex by nature)
sonar.issue.ignore.multicriteria.e3.ruleKey=rust:S3776
sonar.issue.ignore.multicriteria.e3.resourceKey=**/state/**/*.rs

# exclusions
sonar.exclusions=**/target/**,**/fuzz/**,**/static/**,**/*.toml,**/*.md,**/*.yml,**/*.yaml

# test files
sonar.tests=.
sonar.test.inclusions=**/tests/**/*.rs,**/*_test.rs

# Additional source directories for workspace members
sonar.sourceEncoding=UTF-8
```

**Important:** Replace `YOUR_GENERATED_TOKEN_HERE` with the actual token from Step 4.

## Step 6: Run Analysis

From the project root:

```bash
sonar-scanner
```

The scan will:
- Parse Rust source files (some modern syntax may fail to parse with v0.1.4)
- Analyze for code smells, bugs, and security issues
- Import coverage reports if configured
- Upload results to SonarQube

## Troubleshooting

### Plugin API Version Mismatch

**Error:** `Plugin Rust language analyzer requires at least Sonar Plugin API version 10.11.0.2468`

**Fix:** Use Rust plugin v0.1.4 with SonarQube 10.6-community. Do NOT use v0.2.6 or v0.2.7 — they require newer API.

### Parse Errors on Modern Rust Syntax

**Error:** `Unable to parse file` on `async move |...|` or `impl Trait` return types

**Expected:** Plugin v0.1.4 has an older Rust grammar. It will skip files it cannot parse but still analyze the rest. This is a known limitation.

### 401 Unauthorized

**Error:** `Failed to query server version: HTTP 401 Unauthorized`

**Fix:** Verify the token in `sonar-project.properties` is correct and has "Analyze" scope.

### Project Doesn't Exist

**Error:** `You're not authorized to analyze this project or the project doesn't exist`

**Fix:** Create the project in SonarQube UI first (Step 4) with matching project key.

### Elasticsearch/Lucene Codec Errors

**Error:** `fatal exception while booting Elasticsearch` with codec issues

**Fix:** Clean the data volume:
```bash
docker compose down
docker volume rm coincube_sonarqube_data
docker compose up -d
```

## Limitations

- **Rust Plugin v0.1.4** cannot parse modern Rust syntax:
  - `async move |...|` closures
  - `impl Trait` return types in some contexts
  - Complex generic bounds
- Code coverage import requires additional setup (cargo-sonar or similar)

## Version Compatibility Matrix

| SonarQube | Plugin API | Rust Plugin | Status |
|-----------|------------|-------------|--------|
| 10.6-community | 10.7.0.2191 | v0.1.4 | ✅ Working |
| 10.6-community | 10.7.0.2191 | v0.2.6+ | ❌ Requires API 10.11.0.2468 |
| 10.4-community | 10.6.0.2114 | v0.1.4 | ✅ Working |
| 25.x+ | 25.x | Any | ❌ Incompatible (Lucene issues) |

## References

- [Community Rust Plugin Releases](https://github.com/elegoff/sonar-rust/releases)
- [SonarQube Docker Hub](https://hub.docker.com/_/sonarqube)
- [SonarScanner Documentation](https://docs.sonarqube.org/latest/analyzing-source-code/scanners/sonarscanner/)
