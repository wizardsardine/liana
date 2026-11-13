import pytest


def pytest_addoption(parser):
    parser.addoption(
        "--num-shards",
        type=int,
        default=1,
        help="Split the collected tests into this many shards.",
    )
    parser.addoption(
        "--shard-id",
        type=int,
        default=0,
        help="Only run the tests of this shard, from 0 to --num-shards - 1.",
    )


def pytest_collection_modifyitems(config, items):
    num_shards = config.getoption("--num-shards")
    shard_id = config.getoption("--shard-id")
    if not 0 <= shard_id < num_shards:
        raise pytest.UsageError(f"--shard-id must be in [0, {num_shards - 1}]")
    if num_shards == 1:
        return

    # Round-robin so the longer test files are spread over all shards.
    selected = [item for i, item in enumerate(items) if i % num_shards == shard_id]
    deselected = [item for i, item in enumerate(items) if i % num_shards != shard_id]
    config.hook.pytest_deselected(items=deselected)
    items[:] = selected
