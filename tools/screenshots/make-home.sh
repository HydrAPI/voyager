#!/bin/bash
# make-home.sh DIR: the made-up home folder for the README screenshots
# (take.sh): folders, pictures drawn by scene.sh, music, documents and
# downloads (the big ones sparse: they take no space), all with dates in
# the past months, the same each time. DIR is emptied first.
set -e
H=${1:?usage: make-home.sh DIR}
sc=$(cd "$(dirname "$0")" && pwd)/scene.sh
rm -rf "$H"; mkdir -p "$H"; H=$(cd "$H" && pwd); cd "$H"
mkdir -p Desktop Documents Downloads Music Pictures Projects Public Templates Videos
pic() { "$sc" "$@"; }
cd Pictures
pic "Lake at dawn.jpg" '#ffd6a5' '#ff9a76' '#fff3b0' '#7a6c8f' '#4f5d75' '#2d3142' 1
pic "Mountain ridge.jpg" '#caf0f8' '#90e0ef' '#ffffff' '#5c7aa3' '#3d5a80' '#293241' 2
pic "Desert dunes.jpg" '#ffe8d6' '#ffb4a2' '#fff1e6' '#e5989b' '#b5838d' '#6d6875' 3
pic "Harbour lights.jpg" '#14213d' '#3a506b' '#fca311' '#1c2541' '#0b132b' '#050a18' 4
pic "Autumn leaves.jpg" '#fefae0' '#faedcd' '#e76f51' '#d4a373' '#bc6c25' '#7f4f24' 5
pic "Forest path.jpg" '#d8f3dc' '#b7e4c7' '#ffffff' '#52b788' '#2d6a4f' '#1b4332' 6
pic "Meadow.jpg" '#e0fbfc' '#c2dfe3' '#ffd166' '#90be6d' '#6a994e' '#386641' 7
pic "City at night.jpg" '#240046' '#5a189a' '#ffb703' '#3c096c' '#240046' '#10002b' 8
pic "Sea glass.jpg" '#e0f7fa' '#80deea' '#ffffff' '#4dd0e1' '#0097a7' '#006064' 9
pic "Lavender field.jpg" '#f3e8ff' '#e0aaff' '#fff3b0' '#c77dff' '#9d4edd' '#5a189a' 10
mkdir -p Holiday Screenshots Wallpapers
i=11
for n in 2041 2042 2043 2047 2048 2051 2055 2056; do
  pic "Holiday/IMG_$n.jpg" '#bde0fe' '#a2d2ff' '#ffd166' '#8ecae6' '#219ebc' '#023047' $i; i=$((i+1))
done
pic "Wallpapers/Waves.jpg" '#03045e' '#0077b6' '#90e0ef' '#0096c7' '#00b4d8' '#48cae4' 30
magick -size 1200x800 xc:'#f1f3f5' -fill '#ced4da' -draw 'rectangle 0,0 1200,40' -fill white -draw 'rectangle 80,90 1120,740' "Screenshots/Screenshot 2026-11-02.png"
cd ..
cd Music
mkdir -p "Selma Hartwig - Northern Lights" "Night Tram Orchestra - Live" "Field Recordings"
a="Selma Hartwig - Northern Lights"
i=1
for t in Aurora "Cold Harbour" "Paper Boats" "Glass Coast" "Midnight Ferry" "Snowline" "Lanterns" "Home Again"; do
  truncate -s $(( (24 + i * 3) * 1000000 )) "$a/$(printf %02d $i) - $t.flac"; i=$((i+1))
done
pic "$a/cover.jpg" '#0b132b' '#1c2541' '#e0fbfc' '#3a506b' '#5bc0be' '#6fffe9' 40
cd ..
cd Documents
mkdir -p Letters Recipes "Tax 2026" Manuals
printf 'Travel plan\n\n- Train to Hamburg, 8:12\n- Ferry at 14:30\n' > "Travel plan.txt"
truncate -s 18432 "Budget 2026.ods"
truncate -s 24576 "Meeting notes.odt"
truncate -s 96256 "Curriculum vitae.pdf"
truncate -s 31744 "Garden layout.odg"
printf '# Ideas\n\n- Paint the shed\n' > "Ideas.md"
cd ..
cd Downloads
truncate -s 6012954624 ubuntu-26.04-desktop-amd64.iso
truncate -s 48234496 photos-from-anna.zip
truncate -s 1048576 invoice-2026-10.pdf
truncate -s 7340032 podcast-episode-42.mp3
truncate -s 152043520 trailer.mp4
truncate -s 26214 setup-notes.txt
cd ..
cd Projects
mkdir -p garden-planner website "bike trip"
printf 'Fix the gate\nOrder seeds\n' > todo.txt
truncate -s 2048 garden-planner/README.md
cd ..
printf 'Milk\nBread\nCoffee\n' > "Shopping list.txt"
truncate -s 157286400 Videos/"Birthday party.mp4"
truncate -s 82837504 Videos/"Bike trip.mkv"
# Varied dates in the past months, the same each time: in sorted order
# (whatever order the file system lists them in), reversed so that a
# folder's files come before it and it keeps its own date.
base=$(date -d "2026-11-12 18:00" +%s)
i=0
find "$H" -mindepth 1 -print0 | LC_ALL=C sort -rz | while IFS= read -r -d "" f; do
  i=$((i+1)); touch -h -d "@$(( base - (i * 37 % 140) * 86400 - (i * 3517 % 36000) ))" "$f"
done
