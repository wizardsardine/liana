## Lianad blackbox tests

Here we test `lianad` by starting it on a regression testing Bitcoin network,
and by then talking to it as an user would, from the outside.

Python scripts are used for the automation, and specifically the [`pytest` framework](https://docs.pytest.org/en/stable/index.html).

Credits: this test framework was taken and adapted from revaultd, which was itself adapted from
[C-lightning's test framework](https://github.com/ElementsProject/lightning/tree/master/contrib/pyln-testing).

### Building the project for testing

To run the tests, we must build the debug version of `lianad`. Follow the instructions at [`doc/BUILD.md`](../doc/BUILD.md) but instead of running
```
$ cargo build --release
```
Run
```
$ cargo build
```
to build the daemon for testing.  
The `lianad` and `liana-cli` binaries will be in the `target/debug` directory at the root of the
repository.

### Test dependencies

Functional tests dependencies can be installed using `pip`. Use a virtual environment.
```
# Create a new virtual environment, preferably.
python3 -m venv venv
. venv/bin/activate
# Get the deps
pip install -r tests/requirements.txt
```

Additionaly you need to have `bitcoind` installed on your computer, please
refer to [bitcoincore](https://bitcoincore.org/en/download/) for installation. You may use a
specific `bitcoind` binary by specifying the `BITCOIND_PATH` env var.

### Running the tests

From the root of the repository:
```
pytest tests/
```

For running the tests under Taproot a `bitcoind` version 26.0 or superior must be used. It can be
pointed to using the `BITCOIND_PATH` variable. For now, one must also compile the `taproot_signer`
Rust program:
```
(cd tests/tools/taproot_signer && cargo build --release)
```

Then the test suite can be run by using Taproot descriptors instead of P2WSH descriptors by setting
the `USE_TAPROOT` environment variable to `1`.

### Tips and tricks
#### Logging

We use the [Live Logging](https://docs.pytest.org/en/latest/logging.html#live-logs)
functionality from pytest. It is configured in (`pyproject.toml`)[../pyproject.toml] to
output `INFO`-level to the console. If a test fails, the entire `DEBUG` log is output.

You can override the config at runtime with the `--log-cli-level` option:
```
pytest -vvv --log-cli-level=DEBUG -k test_startup
```

Note that we record all logs from daemons, and we start them with `log_level = "debug"`.

#### Running tests in parallel

In order to run tests in parallel, you can use `-n` arg:

```
pytest -n 8 tests/
```
### Test lints

Just use [`black`](https://github.com/psf/black).

### More

See the environment variables in `test_framework/utils.py`.
