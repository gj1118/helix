#!/usr/bin/env bash

REMOTE=https://github.com/tree-sitter/tree-sitter.git
BRANCH=v0.26.9

rm -rf vendor
rm -rf tmp
git clone --depth 1 --branch $BRANCH $REMOTE tmp
mkdir vendor
mv tmp/lib/src vendor
mv tmp/lib/include vendor
mv tmp/LICENSE vendor
rm -rf tmp
