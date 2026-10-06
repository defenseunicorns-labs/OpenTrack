# Sourced by the release signing scripts: an OpenSSL environment that uses
# only the validated OpenSSL 3.0.9 FIPS provider (CMVP #4282), built from the
# Dockerfile's openssl-fips stage and cached. Sets OPENSSL_CONF and
# OPENSSL_MODULES, and stops unless the FIPS provider is the one active.
fips_dir="${XDG_CACHE_HOME:-$HOME/.cache}/opentrack-fips"
if [ ! -f "$fips_dir/fips.so" ] || [ ! -f "$fips_dir/fipsmodule.cnf" ]; then
  echo "building the OpenSSL FIPS provider into $fips_dir" >&2
  root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
  docker buildx build --target openssl-fips-out --output "type=local,dest=$fips_dir" "$root" >&2
fi
cat >"$fips_dir/openssl.cnf" <<CNF
config_diagnostics = 1
openssl_conf = openssl_init
.include $fips_dir/fipsmodule.cnf
[openssl_init]
providers = provider_sect
alg_section = algorithm_sect
[provider_sect]
fips = fips_sect
base = base_sect
[base_sect]
activate = 1
[algorithm_sect]
default_properties = fips=yes
CNF
export OPENSSL_CONF="$fips_dir/openssl.cnf" OPENSSL_MODULES="$fips_dir"
if ! openssl list -providers 2>/dev/null | grep -A3 '^  fips' | grep -q 'status: active'; then
  echo "the OpenSSL FIPS provider is not active" >&2
  exit 1
fi
