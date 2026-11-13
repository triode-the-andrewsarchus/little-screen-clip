// Compiled data-only adapter. No upstream CLI, CSS files or filenames are exposed.
package main

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"runtime"
	"runtime/debug"
	"strings"
	"unicode/utf8"

	"github.com/blampe/goat/ascii"
	"github.com/blampe/goat/svg"
	goatutf8 "github.com/blampe/goat/utf8"
)

const maxSource = 256 * 1024
const maxSVG = 2 * 1024 * 1024

func main() {
	runtime.GOMAXPROCS(2)
	debug.SetMemoryLimit(128 * 1024 * 1024)
	log.SetFlags(0)
	mode := false
	if len(os.Args) == 2 && os.Args[1] == "--utf8" {
		mode = true
	} else if len(os.Args) != 1 {
		log.Fatal("Only the fixed --utf8 mode is supported; source must arrive on stdin")
	}
	source, err := io.ReadAll(io.LimitReader(os.Stdin, maxSource+1))
	if err != nil {
		log.Fatal(err)
	}
	output, err := render(string(source), mode)
	if err != nil {
		log.Fatal(err)
	}
	if _, err := os.Stdout.Write(output); err != nil {
		log.Fatal(err)
	}
}

func render(source string, boxMode bool) ([]byte, error) {
	if len(source) > maxSource || !utf8.ValidString(source) || strings.ContainsRune(source, 0) {
		return nil, errors.New("GoAT source exceeds the byte budget or is not ordinary UTF-8 text")
	}
	if strings.TrimSpace(source) == "" {
		return nil, errors.New("GoAT source is empty")
	}
	lines := strings.Split(strings.TrimSuffix(source, "\n"), "\n")
	if len(lines) > 128 {
		return nil, errors.New("GoAT preview exceeds 128 rows")
	}
	for row, line := range lines {
		if strings.ContainsRune(line, '\t') {
			return nil, fmt.Errorf("GoAT row %d contains a tab; use spaces to preserve grid alignment", row+1)
		}
		if utf8.RuneCountInString(strings.TrimSuffix(line, "\r")) > 256 {
			return nil, errors.New("GoAT preview exceeds 256 columns")
		}
	}
	reserved := ascii.ReservedSet
	if boxMode {
		reserved = goatutf8.ReservedSet
	}
	config, err := svg.NewConfig(reserved, nil)
	if err != nil {
		return nil, err
	}
	input := strings.NewReader(source)
	var canvas svg.AbstractCanvas
	if boxMode {
		canvas = goatutf8.NewCanvas(&config, input)
	} else {
		canvas = ascii.NewCanvas(&config, input)
	}
	var output svgBuffer
	// Native previews have white paper and fixed bundled fonts. Both color
	// schemes deliberately use the same foreground; no caller-supplied CSS.
	svg.WriteCanvas(&config, canvas, true, svg.ColorsOnlyCssFileContent("#111", "#111"), nil, &output)
	return output.data.Bytes(), nil
}

type svgBuffer struct{ data bytes.Buffer }

func (output *svgBuffer) Write(data []byte) (int, error) {
	if len(data) > maxSVG-output.data.Len() {
		return 0, errors.New("GoAT generated SVG exceeds the preview budget")
	}
	return output.data.Write(data)
}
