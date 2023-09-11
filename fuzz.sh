#!/usr/bin/env bash

while true
do
    PROPTEST_CASES=100000 RUST_BACKTRACE=1 cargo test prop_

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
