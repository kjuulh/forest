#!/usr/bin/env zsh

function non-tmp() {
  echo "creating tmp dir"
  dir=$(non tmp)

  echo "moving into: $dir"
  cd "$dir"
}
