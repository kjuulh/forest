#!/usr/bin/env fish

set SCRIPT_NAME (basename (status -f))
set DEFAULT_AGENTS 2
set CLUSTER_PREFIX test-cluster

# Cluster configuration
set -g CLUSTERS \
    "1:8081:8443" \
    "2:8082:8444" \
    "3:8083:8445"

function usage
    echo "Usage: $SCRIPT_NAME <command> [options]"
    echo ""
    echo "Commands:"
    echo "    create    Create k3d test clusters"
    echo "    list      List all k3d clusters"
    echo "    cleanup   Delete test clusters"
    echo ""
    echo "Options:"
    echo "    -h, --help    Show this help message"
    echo ""
    echo "Examples:"
    echo "    $SCRIPT_NAME create"
    echo "    $SCRIPT_NAME list"
    echo "    $SCRIPT_NAME cleanup"
end

function create_cluster
    set -l index $argv[1]
    set -l http_port $argv[2]
    set -l https_port $argv[3]
    set -l cluster_name "$CLUSTER_PREFIX-$index"

    echo "Creating cluster '$cluster_name' (http=$http_port, https=$https_port)..."

    k3d cluster create $cluster_name \
        --agents $DEFAULT_AGENTS \
        --port "$http_port:80@loadbalancer" \
        --port "$https_port:443@loadbalancer" \
        --wait

    if test $status -eq 0
        echo "✓ Created cluster '$cluster_name'"
        return 0
    else
        echo "✗ Failed to create cluster '$cluster_name'"
        return 1
    end
end

function cmd_create
    echo "Starting cluster creation..."

    # Check if k3d is installed
    if not command -q k3d
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    end

    # Start all cluster creations in parallel
    for config in $CLUSTERS
        set -l parts (string split ':' $config)
        set -l index $parts[1]
        set -l http_port $parts[2]
        set -l https_port $parts[3]

        create_cluster $index $http_port $https_port &
    end

    echo "Waiting for all clusters to finish initializing..."

    # Wait for all background jobs to complete
    wait

    # Check if any jobs failed
    if test $status -eq 0
        echo "✓ All clusters have been created successfully!"
    else
        echo "⚠ Warning: Some cluster(s) failed to create"
        exit 1
    end
end

function cmd_list
    if not command -q k3d
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    end

    echo "K3D Clusters:"
    echo "============="
    k3d cluster list
end

function cmd_cleanup
    if not command -q k3d
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    end

    echo "Cleaning up test clusters..."

    # Build list of cluster names
    set -l cluster_names
    for config in $CLUSTERS
        set -l parts (string split ':' $config)
        set -l index $parts[1]
        set -a cluster_names "$CLUSTER_PREFIX-$index"
    end

    # Check which clusters exist
    set -l existing_clusters
    for cluster in $cluster_names
        if k3d cluster list | grep -q "^$cluster"
            set -a existing_clusters $cluster
        end
    end

    if test (count $existing_clusters) -eq 0
        echo "No test clusters found to delete."
        exit 0
    end

    echo "Deleting clusters: $existing_clusters"
    k3d cluster delete $existing_clusters

    if test $status -eq 0
        echo "✓ Cleanup completed"
    else
        echo "✗ Some clusters failed to delete"
        exit 1
    end
end

# Main script logic
function main
    if test (count $argv) -eq 0
        usage
        exit 1
    end

    switch $argv[1]
        case create
            cmd_create
        case list
            cmd_list
        case cleanup
            cmd_cleanup
        case -h --help help
            usage
            exit 0
        case '*'
            echo "Error: Unknown command '$argv[1]'"
            echo
            usage
            exit 1
    end
end

main $argv
