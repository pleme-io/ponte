{
  description = "Bridges LangChain OpenWiki into the pleme-io fleet: reads the target repo's typescape IR + zoekt hits as context, invokes OpenWiki unmodified as a subprocess, routes the result through the fleet's doc/compliance layers.";
  inputs = {
    nixpkgs = {
      follows = "substrate/nixpkgs";
    };
    flake-utils = {
      url = "github:numtide/flake-utils";
    };
    substrate = {
      url = "github:pleme-io/substrate";
    };
  };
  outputs = inputs @ { self, nixpkgs, crate2nix, flake-utils, substrate, devenv, ... }:
    (import "${substrate}/lib/rust-tool-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils devenv;
    }) {
      toolName = "ponte";
      src = self;
      repo = "pleme-io/ponte";
    };
}
