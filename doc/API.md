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
