#!/usr/bin/env zsh

function forest-tmp() {
  echo "creating tmp dir"
  dir=$(forest tmp)

  echo "moving into: $dir"
  cd "$dir"
}
