{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    # Python development (3.12 required by cua-bench)
    python312
    python312Packages.pip
    python312Packages.virtualenv

    # C/C++ libraries needed by numpy and other packages
    stdenv.cc.cc.lib

    # Docker for container operations
    docker
    docker-compose

    # General tools
    git
    curl
    jq
  ];

  # Add library path for numpy and other native extensions
  LD_LIBRARY_PATH = "${pkgs.stdenv.cc.cc.lib}/lib";

  shellHook = ''
    echo "CUA-Bench development environment loaded"
    echo "Python version: $(python3 --version)"

    # Create and activate venv if it doesn't exist
    if [ ! -d ".venv" ]; then
      echo "Creating virtual environment..."
      python3 -m venv .venv
    fi

    source .venv/bin/activate

    # Install package in editable mode if not already installed
    if ! pip show cua-bench > /dev/null 2>&1; then
      echo "Installing cua-bench in editable mode..."
      pip install -e ".[dev]" 2>/dev/null || pip install -e .
    fi

    echo "Virtual environment activated"
  '';
}
