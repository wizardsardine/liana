# Breez SDK Regtest Setup

Install Docker Desktop or Orbstack

Clone this repo:
`git clone https://github.com/breez/breez-sdk-liquid`

Ensure Docker is running
> Note: When using OrbStack on macOS, set 
> `export DOCKER_DEFAULT_PLATFORM=linux/amd64` before starting.

Now:
`cd breez-sdk-liquid/regtest`
Run `git submodule update --init`
Run `./start.sh`


This can take quite a while for the first time, it took me almost an hour to complete, so please be patient.

Once this completes, the local Breez SDK setup is finished.
Optionally, you can verify it by opening `localhost:4002` and `localhost:4003` in your browser, both should display the Esplora Web UI.

Now you can checkout to `feature/breez-active-wallet` branch in Coincube, and you can create a Cube in Regtest Network.

### Sending funds to your Active wallet

After you've created the Cube, the Active Wallet must've been already created for you.
Go to `Active -> Receive`

Enter some random amount (somewhere in the range of 70-80k sats), and some random description and then click on `Generate Invoice`

Copy it to the clipboard.

Run `cd breez-sdk-liquid/regtest`
Run `source boltz/aliases.sh`
Run `lncli-sim 1 payinvoice --force <generated-invoice>`
Run `mine-block` for 2-3 times to simulate mining

Hopefully after this
You will be able to see some balance in your Active Wallet

Once you have funds available, create multiple Cubes and test sending and receiving between the Cubes.

To stop all of the docker containers:
Run `cd breez-sdk-liquid/regtest`
Run `./stop.sh`

This will stop all of the containers and the data associated with it.


### Vault Setup

If you want to test Vault functionality locally along with Active Wallet, then you need to install Bitcoin Core on your machine


To install it:
on macOS: Run `brew install bitcoin`

For other OS: 

Download Bitcoin Core from https://bitcoincore.org/bin/bitcoin-core-30.1/ for your operating system.
Extract the archive, then take the binaries from the extracted `bin/` directory and place them in a location that is included in your system `PATH`.

Verify it by running `bitcoind --version`

Create a `bitcoin.conf` file in the specified path.

macOS: `~/Library/Application Support/Bitcoin/bitcoin.conf`
Linux: `~/.bitcoin/bitcoin.conf`
Windows: `C:\Users\<username>\AppData\Local\Bitcoin\bitcoin.conf`

Paste this content into the `bitcoin.conf` file
```
regtest=1
[regtest]
server=1
fallbackfee=0.0001 # for regtest only
rpcuser=user
rpcpassword=password
rpcallowip=0.0.0.0/0
txindex=1
rpcport=28443
listen=1
port=28444
addnode=127.0.0.1:18444
```

In a new terminal window
Run `bitcoind` 

> Ensure that docker containers are running before running `bitcoind`


Proceed for Vault Creation, do the process as usual, but in Bitcoin Node Management
Choose -> I already have a node
Enter Address as: `127.0.0.1:28443`

Choose RPC auth as User and password
These are the credentials
User: `user`
Password: `password`