# Services

This document describes the data flow for the three external services used by liana-gui:
- **Connect Service**: Authentication and remote wallet backend
- **Fiat Service**: Bitcoin price fetching
- **Keys Service**: Provider key fetching and redemption

---

## Connect Service

The Connect service provides authentication and a remote wallet backend for Liana.
It enables users to manage wallets through a Liana Connect service instead of running
a local daemon.

### Components

- **AuthClient** (`services/connect/client/auth.rs`): Handles OTP-based authentication
- **BackendClient** (`services/connect/client/backend/mod.rs`): Manages connection to
  the remote backend API
- **BackendWalletClient**: Wallet-specific operations (implements `Daemon` trait)
- **Cache** (`services/connect/client/cache.rs`): Local file-based token storage
- **LianaLiteLogin** (`services/connect/login.rs`): UI flow for login/connection

### Authentication Flow

The authentication flow uses OTP (One-Time Password) via email:

```
User                    GUI                    AuthClient              Auth API
|                        |                             |                         |
|--- Enter Email ------->|                             |                         |
|                        |-- sign_in_otp() ----------->|                         |
|                        |                             |-- POST /auth/v1/otp --->|
|                        |                             |<-- 200 OK --------------|
|                        |<-- Success -----------------|                         |
|<-- Check Email --------|                             |                         |
|                        |                             |                         |
|--- Enter OTP --------->|                             |                         |
|                        |-- verify_otp(token) ------->|                         |
|                        |                             |-- POST /auth/v1/verify->|
|                        |                             |<-- AccessTokenResponse--|
|                        |<-- AccessTokenResponse -----|                         |
|                        |                             |                         |
|                        |-- BackendClient::connect()->|                         |
|                        |                             |-- GET /v1/me ---------->|
|                        |                             |<-- User Claims ---------|
|                        |<-- BackendClient -----------|                         |
```

### Backend Connection Flow

After authentication, the client connects to the backend and performs wallet operations:

```
GUI                   BackendClient                             Backend API
|                          |                                         |
|-- connect() ------------>|                                         |
|                          |-- GET /v1/me -------------------------->|
|                          |<-- User Claims -------------------------|
|<-- BackendClient --------|                                         |
|                          |                                         |
|-- list_wallets() ------->|                                         |
|                          |-- GET /v1/wallets --------------------->|
|                          |<-- Wallet List -------------------------|
|<-- Wallets --------------|                                         |
|                          |                                         |
|-- connect_wallet() ----->|                                         |
|                          |                                         |
|-- get_info() ----------->|                                         |
|                          |-- GET /v1/wallets/{id} ---------------->|
|                          |<-- Wallet Info -------------------------|
|<-- Wallet Info ----------|                                         |
|                          |                                         |
|-- list_coins() --------->|                                         |
|                          |-- GET /v1/wallets/{id}/coins ---------->|
|                          |<-- Coins List --------------------------|
|<-- Coins ----------------|                                         |
|                          |                                         |
|-- create_spend_tx() ---->|                                         |
|                          |-- POST /v1/wallets/{id}/psbts/generate->|
|                          |<-- Draft PSBT --------------------------|
|<-- PSBT -----------------|                                         |
```

### Token Refresh Flow

Access tokens expire and need to be refreshed periodically:

```
BackendWalletClient      AuthClient                Auth API
|                           |                        |
|--- is_alive() ----------->|                        |
|                           |                        |
|-- Check token expiry ---->|                        |
|                           |                        |
|<-- Token expired? --------|                        |
|                           |                        |
|-- refresh_token() ------->|                        |
|                           |-- POST /auth/v1/token->|
|                           |<-- New Tokens ---------|
|<-- New Tokens ------------|                        |
|                           |                        |
|-- update_connect_cache()->|                        |
|                           |                        |
|                           |                        |
|<-- Updated Tokens --------|                        |
```

Authentication tokens are cached locally in `connect.json`:

```
Application           Cache Module          File System
|                          |                      |
|-- Load credentials ----->|                      |
|                          |-- Read connect.json->|
|                          |<-- Account Data -----|
|<-- Cached Tokens --------|                      |
|                          |                      |
|-- Update tokens -------->|                      |
|                          |-- Lock file -------->|
|                          |<- Read file ---------|
|                          |-- Update data -------|
|                          |-- Write file ------->|
|                          |-- Unlock file ------>|
|<-- Success --------------|                      |
```

### Token Authentication

All backend API requests are authenticated using Bearer token authentication.
The access token obtained during the OTP verification flow is included in the 
`Authorization` header for every request to the backend API.

**Request Headers:**
- `Authorization: Bearer {access_token}` - The access token from `AccessTokenResponse`
- `Content-Type: application/json`
- `Liana-Version: {version}` - Client version
- `User-Agent: liana-gui/{version}` - Client identifier

**Token Usage Flow:**

```
BackendClient                    Backend API
|                                    |
|-- Read access_token from cache  -->|
|                                    |
|-- Build HTTP request --------------|
|   Authorization: Bearer {token}    |
|   Content-Type: application/json   |
|   Liana-Version: {version}         |
|   User-Agent: liana-gui/{version}  |
|                                    |
|-- Send request ------------------->|
|                                    |
|<-- Response -----------------------|
|                                    |
```

**Token Refresh:**
- Tokens are automatically refreshed when they expire (checked via `is_alive()`)
- If a request returns 401 Unauthorized, the client marks itself as unauthenticated
- The refresh token is used to obtain a new access token without requiring user
  interaction

**Auth API Requests:**
- Auth API requests (OTP, verify, refresh) use a different authentication method
- They include an `apikey` header instead of Bearer token authentication
- The API key is a public key fetched from the backend service at runtime
  via `GET /v1/desktop`
- The service provides this key to identify the client application; it is not 
  hardcoded or client-generated

## Fiat Service

The Fiat service fetches Bitcoin prices from external APIs to display fiat
currency equivalents in the GUI.

### Components

- **PriceClient** (`services/fiat/client.rs`): HTTP client wrapper for price APIs
- **PriceSource** (`services/fiat/source.rs`): Implementation for different 
  price sources (CoinGecko, MempoolSpace)
- **PriceApi** (`services/fiat/api.rs`): Trait defining price fetching interface
- **GlobalCache** (`gui/cache.rs`): Global in-memory cache with TTL
- **FiatPriceRequest** (`app/cache.rs`): Request structure with source and currency

### Price Fetching Flow

Prices are fetched on-demand when needed and cached for 5 minutes:

```
GUI                    GlobalCache             PriceClient          External API
|                          |                        |                        |
|-- Check cache ---------->|                        |                        |
|                          |                        |                        |
|<-- Cache miss -----------|                        |                        |
|                          |                        |                        |
|-- Request price -------->|                        |                        |
|                          |-- Mark pending ------->|                        |
|                          |                        |                        |
|                          |                        |-- GET price URL ------>|
|                          |                        |<-- JSON Response ------|
|                          |                        |                        |
|                          |                        |-- Parse response ----->|
|                          |                        |<-- Price Data ---------|
|                          |<-- FiatPrice ----------|                        |
|                          |                        |                        |
|                          |-- Store in cache ----->|                        |
|<-- FiatPrice ------------|                        |                        |
|                          |                        |                        |
|-- Use price in UI ------>|                        |                        |
```

### Periodic Price Updates

The GUI periodically checks for stale prices and fetches updates:

```
GUI (Tick Event)       GlobalCache             PriceClient          External API
|                          |                        |                        |
|-- On tick -------------->|                        |                        |
|                          |                        |                        |
|-- Check all wallets ---->|                        |                        |
|                          |                        |                        |
|-- For each wallet:       |                        |                        |
|   Check fiat setting --->|                        |                        |
|                          |                        |                        |
|-- Is price stale? ------>|                        |                        |
|                          |                        |                        |
|<-- Yes, stale -----------|                        |                        |
|                          |                        |                        |
|-- Create request ------->|                        |                        |
|                          |-- Mark pending ------->|                        |
|                          |                        |                        |
|                          |                        |-- GET price URL ------>|
|                          |                        |<-- JSON Response ------|
|                          |                        |                        |
|                          |                        |-- Parse response ----->|
|                          |                        |<-- Price Data ---------|
|                          |<-- FiatPrice ----------|                        |
|                          |                        |                        |
|                          |-- Update cache ------->|                        |
|<-- Price updated --------|                        |                        |
|                          |                        |                        |
|-- Update UI ------------>|                        |                        |
```

### Currency Listing Flow

When configuring fiat settings, available currencies are fetched:

```
Settings UI            GlobalCache             PriceClient          External API
|                          |                        |                        |
|-- Select source -------->|                        |                        |
|                          |                        |                        |
|-- Check cache ---------->|                        |                        |
|                          |                        |                        |
|<-- Cache miss -----------|                        |                        |
|                          |                        |                        |
|-- Request currencies --->|                        |                        |
|                          |                        |-- GET currencies URL ->|
|                          |                        |<-- JSON Response ------|
|                          |                        |                        |
|                          |                        |-- Parse response ----->|
|                          |                        |<-- Currency List ------|
|                          |<-- Currencies ---------|                        |
|                          |                        |                        |
|                          |-- Cache (1 hour TTL) ->|                        |
|<-- Currency List --------|                        |                        |
|                          |                        |                        |
|-- Display in dropdown -->|                        |                        |
```

### Price Source Implementation

Different price sources have different API formats:

- **CoinGecko**: `https://api.coingecko.com/api/v3/exchange_rates`
  - Returns rates object with currency codes as keys
  - No timestamp in response
  
- **MempoolSpace**: `https://mempool.space/api/v1/prices`
  - Returns object with currency codes as keys, values in satoshis
  - Includes timestamp field

### Caching Strategy

- **Price Cache TTL**: 5 minutes (300 seconds)
- **Currency List TTL**: 1 hour (3600 seconds)
- **Pending Requests**: Tracked to avoid duplicate requests
- **Cache Key**: `(PriceSource, Currency)` tuple

---

## Keys Service

The Keys service fetches and redeems provider keys (SafetyNet or Cosigner keys)
from a remote service. These keys are used during wallet installation.

### Components

- **Client** (`services/keys/mod.rs`): HTTP client for keys API
- **Key API** (`services/keys/api.rs`): Data structures for keys
- **Installer** (`installer/step/descriptor/editor/key.rs`): Uses keys service
  during wallet setup


### Key Fetching Flow

When a user enters a token to fetch a provider key:

```
Installer              KeysClient                  Keys API
|                          |                           |
|-- User enters token ---->|                           |
|                          |-- GET /v1/keys?token=xxx->|
|                          |<-- Key JSON --------------|
|<-- Key (or Error) -------|                           |
```

### Key Redemption Flow

When redeeming a key (marking it as used):

```
Installer              KeysClient                         Keys API
|                            |                                |
|-- Redeem key ------------->|                                |
|                            |-- POST /v1/keys/{uuid}/redeem->|
|                            |<-- Redeemed Key ---------------|
|<-- Key (or Error) ---------|                                |
```

### Token Authentication

The Keys service uses token-based authentication where the token is provided by the
user during wallet installation. The token is used to authenticate requests to fetch
and redeem keys.

**Token Usage:**

1. **GET Requests (Fetch Key):**
   - Token is passed as a query parameter: `GET /v1/keys?token={token}`
   - Example: `GET /v1/keys?token=123-word1-word2-word3`

2. **POST Requests (Redeem Key):**
   - Token is included in the JSON request body: `{ "token": "..." }`
   - Example: `POST /v1/keys/{uuid}/redeem` with body `{"token": "123-word1-word2-word3"}`

**Request Headers:**
- `Content-Type: application/json`
- `API-Version: 0.1`
- `User-Agent: liana-gui/{version}`

**Token Format:**
- Tokens are user-provided strings in the format: `{number}-{word1}-{word2}-{word3}`
- The token serves as both authentication and authorization to access specific keys

### Key Data Structure

Keys returned from the API contain:
- `provider`: Provider information (UUID, name)
- `uuid`: Key UUID
- `kind`: KeyKind (SafetyNet or Cosigner)
- `status`: KeyStatus (NotFetched, Fetched, Redeemed)
- `xpub`: DescriptorPublicKey (the actual key)

### API Endpoints

- **Base URL**: `https://keys.wizardsardine.com`
- **Get Key**: `GET /v1/keys?token={token}`
- **Redeem Key**: `POST /v1/keys/{uuid}/redeem` with `{ "token": "..." }`
