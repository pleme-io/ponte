{
  description = "Bridges LangChain OpenWiki into the pleme-io fleet: reads the target repo's typescape IR + zoekt hits as context, invokes OpenWiki unmodified as a subprocess, routes the result through the fleet's doc/compliance layers.";

  # substrate.rust.tool dispatches over Cargo.gen.lock (the slim gen delta,
  # reconstructed to the full BuildSpec in pure Nix) — no crate2nix, no Cargo.nix.
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool {
    src = ./.;
  };
}
