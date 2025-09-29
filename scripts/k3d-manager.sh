#!/usr/bin/env bash

set -e

SCRIPT_NAME=$(basename "$0")
DEFAULT_AGENTS=2
CLUSTER_PREFIX="test-cluster"

# Cluster configuration
CLUSTERS=(
    "1:8081:8443"
    "2:8082:8444"
    "3:8083:8445"
)

usage() {
    cat <<EOF
Usage: $SCRIPT_NAME <command> [options]

Commands:
    create    Create k3d test clusters
    list      List all k3d clusters
    cleanup   Delete test clusters

Options:
    -h, --help    Show this help message

Examples:
    $SCRIPT_NAME create
    $SCRIPT_NAME list
    $SCRIPT_NAME cleanup
EOF
}

create_cluster() {
    local index=$1
    local http_port=$2
    local https_port=$3
    local cluster_name="${CLUSTER_PREFIX}-${index}"

    echo "Creating cluster '${cluster_name}' (http=${http_port}, https=${https_port})..."

    k3d cluster create "${cluster_name}" \
        --agents ${DEFAULT_AGENTS} \
        --port "${http_port}:80@loadbalancer" \
        --port "${https_port}:443@loadbalancer" \
        --wait

    echo "✓ Created cluster '${cluster_name}'"
}

cmd_create() {
    echo "Starting cluster creation..."

    # Check if k3d is installed
    if ! command -v k3d &> /dev/null; then
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    fi

    # Start all cluster creations in parallel
    local pids=()
    for config in "${CLUSTERS[@]}"; do
        IFS=':' read -r index http_port https_port <<< "$config"
        create_cluster "$index" "$http_port" "$https_port" &
        pids+=($!)
    done

    echo "Waiting for all clusters to finish initializing..."

    # Wait for all background processes
    local failed=0
    for pid in "${pids[@]}"; do
        if ! wait "$pid"; then
            failed=$((failed + 1))
        fi
    done

    if [ $failed -eq 0 ]; then
        echo "✓ All clusters have been created successfully!"
    else
        echo "⚠ Warning: $failed cluster(s) failed to create"
        exit 1
    fi
}

cmd_list() {
    if ! command -v k3d &> /dev/null; then
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    fi

    echo "K3D Clusters:"
    echo "============="
    k3d cluster list
}

cmd_cleanup() {
    if ! command -v k3d &> /dev/null; then
        echo "Error: k3d is not installed. Please install k3d first."
        exit 1
    fi

    echo "Cleaning up test clusters..."

    # Build list of cluster names
    local cluster_names=()
    for config in "${CLUSTERS[@]}"; do
        IFS=':' read -r index _ _ <<< "$config"
        cluster_names+=("${CLUSTER_PREFIX}-${index}")
    done

    # Check which clusters exist
    local existing_clusters=()
    for cluster in "${cluster_names[@]}"; do
        if k3d cluster list | grep -q "^${cluster}"; then
            existing_clusters+=("$cluster")
        fi
    done

    if [ ${#existing_clusters[@]} -eq 0 ]; then
        echo "No test clusters found to delete."
        exit 0
    fi

    echo "Deleting clusters: ${existing_clusters[*]}"
    k3d cluster delete "${existing_clusters[@]}"

    echo "✓ Cleanup completed"
}

# Main script logic
main() {
    if [ $# -eq 0 ]; then
        usage
        exit 1
    fi

    case "$1" in
        create)
            cmd_create
            ;;
        list)
            cmd_list
            ;;
        cleanup)
            cmd_cleanup
            ;;
        -h|--help|help)
            usage
            exit 0
            ;;
        *)
            echo "Error: Unknown command '$1'"
            echo
            usage
            exit 1
            ;;
    esac
}

main "$@"
