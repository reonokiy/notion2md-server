variable "image" {
  default = "latest"
}

group "default" {
  targets = ["notion2md-server"]
}

target "notion2md-server" {
  context    = "."
  dockerfile = "Dockerfile"
  tags       = ["${image}"]
  platforms  = ["linux/amd64", "linux/arm64"]
  cache-from = ["type=gha"]
  cache-to   = ["type=gha,mode=max"]
}
