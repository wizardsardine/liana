## Lianad blackbox tests

Here we test `lianad` by starting it on a regression testing Bitcoin network,
and by then talking to it as an user would, from the outside.

Python scripts are used for the automation, and specifically the [`pytest` framework](https://docs.pytest.org/en/stable/index.html).

Credits: this test framework was taken and adapted from revaultd, which was itself adapted from
[C-lightning's test framework](https://github.com/ElementsProject/lightning/tree/master/contrib/pyln-testing).

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

### Test lints

Just use [`black`](https://github.com/psf/black).

### More

See the environment variables in `test_framework/utils.py`.
