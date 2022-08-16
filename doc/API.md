# Minisafe API

`minisafe` exposes a [JSON-RPC 2.0](https://www.jsonrpc.org/specification)
interface over a Unix Domain socket.

Commands must be sent as valid JSONRPC 2.0 requests, ending with a `\n`.

| Command                                                     | Description                                          |
| ----------------------------------------------------------- | ---------------------------------------------------- |
| [`stop`](#stop)                                             | Stops the minisafe daemon                            |
| [`getinfo`](#getinfo)                                       | Get general information about the daemon             |
| [`getnewaddress`](#getnewaddress)                           | Get a new receiving address                          |

# Reference

## General

### `stop`

Stops the minisafe daemon.

#### Response

Returns an empty response.

| Field         | Type   | Description |
| ------------- | ------ | ----------- |

### `getinfo`

General information about the daemon

#### Request

This command does not take any parameter for now.

| Field         | Type              | Description                                                 |
| ------------- | ----------------- | ----------------------------------------------------------- |

#### Response

| Field                | Type    | Description                                                                                  |
| -------------------- | ------- | -------------------------------------------------------------------------------------------- |
| `version`            | string  | Version following the [SimVer](http://www.simver.org/) format                                |
| `network`            | string  | Answer can be `mainnet`, `testnet`, `regtest`                                                |
| `blockheight`        | integer | Current block height                                                                         |
| `sync`               | float   | The synchronization progress as percentage (`0 < sync < 1`)                                  |
| `descriptors`        | object  | Object with the name of the descriptor as key and the descriptor string as value             |

### `getnewaddress`

Get a new address for receiving coins. This will always generate a new address regardless of whether
it was used or not.

#### Request

This command does not take any parameter for now.

| Field         | Type              | Description                                                 |
| ------------- | ----------------- | ----------------------------------------------------------- |

#### Response

| Field         | Type   | Description        |
| ------------- | ------ | ------------------ |
| `address`     | string | A Bitcoin address  |


### `listcoins`

List our current Unspent Transaction Outputs.

#### Request

This command does not take any parameter for now.

| Field         | Type              | Description                                                 |
| ------------- | ----------------- | ----------------------------------------------------------- |

#### Response

| Field          | Type          | Description                                                      |
| -------------- | ------------- | ---------------------------------------------------------------- |
| `amount`       | int           | Value of the UTxO in satoshis                                    |
| `outpoint`     | string        | Transaction id and output index of this coin                     |
| `block_height` | int or null   | Blockheight the transaction was confirmed at, or `null`          |
