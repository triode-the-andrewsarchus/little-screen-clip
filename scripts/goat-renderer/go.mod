module little-clip.local/goat-renderer

go 1.25

require github.com/blampe/goat v0.5.1
require github.com/tdewolff/parse/v2 v2.7.19 // indirect

replace github.com/blampe/goat => ../../vendor/goat
replace github.com/tdewolff/parse/v2 => ../../vendor/tdewolff-parse
