{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    rustup
    pkg-config
    openssl
    awscli2
    rclone
    fuse3
    git
  ];

  shellHook = ''
    echo "provi dev shell ready"
    echo "available: cargo, rustc, aws, rclone, fusermount"
    echo "cargo: $(cargo --version 2>/dev/null || echo 'run: rustup default stable')"
    echo "rustc: $(rustc --version 2>/dev/null || echo 'run: rustup default stable')"
  '';
}
