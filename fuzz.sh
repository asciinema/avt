#!/usr/bin/env bash

while true
do
    QUICKCHECK_TESTS=1000000 RUST_BACKTRACE=1 cargo test

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
