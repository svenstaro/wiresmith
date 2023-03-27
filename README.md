# wiresmith - Auto-config Wireguard clients into a mesh

## Development

This project uses Podman in rootless mode to facilitate rapid local testing.
Make sure you have [just](https://github.com/casey/just) and [zellij](https://zellij.dev/)
installed locally and then run either `just test` for automatic testing or `just interactive` for
interactive testing. The interactive session will spawn two systemds in containers and then run one
instance of `wiresmith` in each of them so you can watch them and see how they interact.
