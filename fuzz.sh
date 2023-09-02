#!/usr/bin/env bash

while true
do
    PROPTEST_CASES=1000000 RUST_BACKTRACE=1 cargo test prop_

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
