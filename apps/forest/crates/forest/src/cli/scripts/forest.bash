# forest shell bash — interactive helpers

forest-tmp() {
  echo "creating tmp dir"
  dir=$(forest tmp)

  echo "moving into: $dir"
  cd "$dir"
}
