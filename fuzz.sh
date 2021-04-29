#!/usr/bin/env bash

while true
do
    QUICKCHECK_TESTS=1000000 RUST_BACKTRACE=1 cargo test qc_

    if [[ x$? != x0 ]] ; then
        exit $?
    fi
done
