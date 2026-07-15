# forest shell fish — interactive helpers

function forest-tmp
    echo "creating tmp dir"
    set dir (forest tmp)

    echo "moving into: $dir"
    cd "$dir"
end
