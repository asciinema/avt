#!/usr/bin/env bash

test=${1:-prop_}

while true
do
    PROPTEST_CASES=100000 RUST_BACKTRACE=1 cargo test $test

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
