#!/usr/bin/env bash

test=${1:-prop_}

export CARGO_PROFILE_TEST_OPT_LEVEL=3

while true
do
    PROPTEST_CASES=1000000 RUST_BACKTRACE=1 cargo test $test

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
