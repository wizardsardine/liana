#!/bin/bash

iteration=0

while true; do
    iteration=$((iteration+1))

    pytest -vvvv --log-cli-level=DEBUG $1

    if [ $? -ne 0 ]; then
        echo "Test failed on iteration $iteration!"
        break
    fi
done
