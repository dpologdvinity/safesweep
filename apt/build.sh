#!/usr/bin/env bash
# Builds a signed apt repository from .deb files, ready to be served by GitHub Pages.
#
# Usage:     apt/build.sh DEBS OUT KEY_FINGERPRINT
#              DEBS             directory holding the .deb files (amd64 and arm64)
#              OUT              directory to create the repository in
#              KEY_FINGERPRINT  gpg key (already in the keyring) that signs the repository
# Needs:     apt-ftparchive (package apt-utils), gpg, gzip
# Produces:  OUT/pool/main/*.deb                         the packages
#            OUT/dists/stable/main/binary-ARCH/Packages  the package lists (plain and .gz)
#            OUT/dists/stable/{Release,InRelease,Release.gpg}  the index, signed two ways
#            OUT/key.asc  the public key users add;  OUT/index.html  setup instructions
#
# The repository has one suite (`stable`) and one component (`main`). apt verifies the signature
# on `InRelease`, which in turn lists the checksum of every Packages file, which lists the
# checksum of every .deb: so tampering with any file is detected.
set -euo pipefail
debs=${1:?usage: apt/build.sh DEBS OUT KEY_FINGERPRINT} out=${2:?} key=${3:?}
url=https://dpologdvinity.github.io/safesweep
mkdir -p "$out/pool/main" "$out/dists/stable/main/binary-amd64" "$out/dists/stable/main/binary-arm64"
cp "$debs"/*.deb "$out/pool/main/"
cd "$out"
# One package list per architecture; the .gz copy is what apt downloads by default.
for arch in amd64 arm64; do
  apt-ftparchive --arch "$arch" packages pool > "dists/stable/main/binary-$arch/Packages"
  gzip -9kf "dists/stable/main/binary-$arch/Packages"
done
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=safesweep \
  -o APT::FTPArchive::Release::Label=safesweep \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
  -o APT::FTPArchive::Release::Components=main \
  release dists/stable > Release.tmp # written outside dists/ so it does not index itself
mv Release.tmp dists/stable/Release
# Sign: InRelease is the clear-signed form modern apt reads; Release + Release.gpg is the older
# detached form, kept for old clients.
gpg --batch --yes --local-user "$key" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$key" --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
gpg --batch --armor --export "$key" > key.asc # the public key only
# A static page with the setup commands, shown at the site's root.
cat > index.html <<HTML
<!doctype html><meta charset="utf-8"><title>safesweep apt repository</title>
<h1>safesweep apt repository</h1>
<pre>
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL $url/key.asc | sudo tee /etc/apt/keyrings/safesweep.asc >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/safesweep.asc] $url stable main" | sudo tee /etc/apt/sources.list.d/safesweep.list
sudo apt update &amp;&amp; sudo apt install safesweep
</pre>
<p>Key fingerprint: <code>$key</code></p>
<p><a href="https://github.com/dpologdvinity/safesweep">github.com/dpologdvinity/safesweep</a></p>
HTML
touch .nojekyll # serve dists/ and pool/ as-is
