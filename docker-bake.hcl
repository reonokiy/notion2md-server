group "default" {
  targets = ["notion2md-server"]
}

target "notion2md-server" {
  context    = "."
  dockerfile = "Dockerfile"
  # Tags are injected by the workflow via --set notion2md-server.tags=...
  tags       = []
  platforms  = ["linux/amd64", "linux/arm64"]
  cache-from = ["type=gha"]
  cache-to   = ["type=gha,mode=max"]
}
