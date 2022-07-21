# Minisafe API

`minisafe` exposes a [JSON-RPC 2.0](https://www.jsonrpc.org/specification)
interface over a Unix Domain socket.

Note that all addresses are bech32-encoded *version 0* native Segwit `scriptPubKey`s.

| Command                                                     | Description                                          |
| ----------------------------------------------------------- | ---------------------------------------------------- |
| [`help`](#help)                                             | Display all available commands                       |
| [`stop`](#stop)                                             | Stops the minisafe daemon                            |
| [`getinfo`](#getinfo)                                       | Display general information                          |
| [`getaddress`](#getaddress)                                 | Get an address                                       |
| [`listutxos`](#listutxos)                                   | Display a paginated list of utxos                    |
| [`getspendtx`](#getspendtx)                                 | Retrieve the Minisafe spend transaction to sign      |
| [`updatespendtx`](#updatespendtx)                           | Update the Minisafe spend transaction                |
| [`deletespendtx`](#deletespendtx)                           | Delete the Minisafe spend transaction                |
| [`listspendtxs`](#listspendtxs)                             | Retrieve the Minisafe pending outgoing transactions  |
| [`gethistory`](#gethistory)                                 | Retrieve the wallet history                          |
| [`broadcast`](#broadcast)                                   | Broadcast a Spend transaction                        |

# Reference

## General

### `help`

Display all available commands.

#### Response

| Field      | Type   | Description                                                                                                      |
| ---------- | ------ | ---------------------------------------------------------------------------------------------------------------- |
| `commands` | object | One entry per command, specifying the command name and parameters. Optional parameters are enclosed in brackets. |

### `stop`

Stops the minisafe daemon.

### `getinfo`

Display general information about the current daemon state.

#### Response

| Field                | Type    | Description                                                                                  |
| -------------------- | ------- | -------------------------------------------------------------------------------------------- |
| `blockheight`        | integer | Current block height                                                                         |
| `network`            | string  | Answer can be `mainnet`, `testnet`, `regtest`                                                |
| `sync`               | float   | The synchronization progress as percentage (`0 < sync < 1`)                                  |
| `version`            | string  | Version following the [SimVer](http://www.simver.org/) format                                |

### `getaddress`

Get an address.

#### Response

| Field         | Type              | Description                                                 |
| ------------- | ----------------- | ----------------------------------------------------------- |
| `index`       | string (optional) | Get a deposit address for a specific derivation index       |


#### Response

| Field         | Type   | Description |
| ------------- | ------ | ----------- |
| `address`     | string | An address  |


## `listutxos`

The `listutxos` RPC command displays a list of unspent transaction
outputs.

### Request

| Parameter   | Type         | Description                                                                                     |
| ----------- | ------------ | ----------------------------------------------------------------------------------------------- |

#### Response

| Field         | Type                                      | Description                |
| ------------- | ----------------------------------------- | -------------------------- |
| `utxos`       | array of [Utxo resource](#utxo-resource)  | Unspent transaction output |

##### UTXO resource

| Field      | Type         | Description                                |
| ---------- | ------------ | ------------------------------------------ |
| `amount`   | int          | Amount in satoshis                         |
| `outpoint` | string       | Outpoint of the output                     |

### `getspendtx`

The `getspendtx` RPC Command builds and returns the spend transaction given a
set of utxos to spend.

#### Request

| Parameter   | Type                 | Description                        |
| ----------- | -------------------- | ---------------------------------- |
| `inputs`    | string array         | Utxo outpoints -- optional         |
| `outputs`   | map of string to int | Map of Bitcoin addresses to amount |
| `feerate`   | int                  | Target feerate for the transaction |

#### Response

| Field      | Type                                                        | Description                    |
| ---------- | ----------------------------------------------------------- | ------------------------------ |
| `spend_tx` | [Spend transaction resources](#spend_transaction_resources) | Spend transaction informations |

### `updatespendtx`

The `updatespendtx` RPC Command stores or update the stored Spend transaction with the
given one. The signatures from both the old & the new transactions will be merged.

#### Request

| Field       | Type         | Description                                                           |
| ----------- | ------------ | --------------------------------------------------------------------- |
| `spend_tx`  | string       | Base64-encoded Spend transaction PSBT                                 |

#### Response

None; the `result` field will be set to the empty object `{}`. Any value should be
disregarded for forward compatibility.


### `delspendtx`

#### Request

| Field          | Type   | Description                                         |
| -------------- | ------ | --------------------------------------------------- |
| `spend_txid`   | string | Hex encoded txid of the Spend transaction to delete |

#### Response

None; the `result` field will be set to the empty object `{}`. Any value should be
disregarded for forward compatibility.


### `listspendtxs`

List spend transaction that are not totally signed or are not broadcasted.

#### Request

| Field          | Type   | Description                                                          |
| -------------- | ------ | -------------------------------------------------------------------- |

#### Response

| Field          | Type   | Description                                                          |
| -------------- | ------ | -------------------------------------------------------------------- |
| `spend_txs`    | array  | Array of [Spend transaction resources](#spend_transaction_resources) |

##### Spend transaction resources

| Field               | Type         | Description                                |
| ------------------- | ------------ | ------------------------------------------ |
| `psbt`              | string       | Base64-encoded Spend transaction PSBT      |
| `change_index`      | integer      | Index of the change output, might be null  |

### `broadcast`

The `broadcast` RPC Command build and broadcast the transaction out of
a signed psbt.

#### Request

| Parameter   | Type    | Description                           |
| ----------- | ------- | ----------------------------------    |
| `psbt`      | string  | Base64-encoded Spend transaction PSBT |

#### Response

| Field      | Type                                                        | Description                    |
| ---------- | ----------------------------------------------------------- | ------------------------------ |

### `gethistory`

`gethistory` retrieves a paginated list of accounting events.

Aiming at giving an accounting point of view, the amounts returned by this call are the total
of inflows and outflows net of any change amount (that is technically a transaction output, but not a cash outflow).

#### Request

| Field         | Type         | Description                                                          |
| ------------- | ------------ | -------------------------------------------------------------------- |
| `kind`        | string array | Type of the events to retrieve, can be `deposit`, `cancel`, `spend`  |
| `start`       | int          | Timestamp of the beginning of the period to retrieve events for      |
| `end`         | int          | Timestamp of the end of the period to retrieve events for            |
| `limit`       | int          | Maximum number of events to retrieve                                 |

#### Response

| Field          | Type   | Description                                |
| -------------- | ------ | ------------------------------------------ |
| `events`       | array  | Array of [Event resource](#event-resource) |

##### Event Resource

| Field         | Type          | Description                                                                                                             |
| ------------- | ------------- | -----------------------------------------------------------------------------------------------------------------       |
| `blockheight` | int           | Blockheight of the event final transaction                                                                              |
| `txid`        | string        | Hex string  of the event final transaction id                                                                           |
| `kind`        | string        | Type of the event. Can be `deposit`, `spend`                                                                  |
| `date`        | int           | Timestamp of the event                                                                                                  |
| `amount`      | int or `null` | Absolute amount in satoshis that is entering or exiting the wallet, `null` if the event is a `cancel` event             |
