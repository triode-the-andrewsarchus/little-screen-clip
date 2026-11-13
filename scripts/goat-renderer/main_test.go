package main

import (
	"bytes"
	"encoding/xml"
	"io"
	"os"
	"strings"
	"testing"
)

func TestAsciiAndUnicodeGeometryUsesTheUpstreamRenderers(t *testing.T) {
	for _, test := range []struct {
		source string
		box    bool
	}{
		{"    .------------.\no-->|Hello, world +--*\n    '------------'\n", false},
		{"    ╭────────────┐\n○──►│Hello, world├─●\n    └────────────╯\n", true},
	} {
		result, err := render(test.source, test.box)
		if err != nil {
			t.Fatal(err)
		}
		count := svgElements(t, result)
		if count["image"] != 0 || count["script"] != 0 {
			t.Fatal("executable/external artwork")
		}
		shapes := count["path"] + count["polyline"] + count["polygon"] + count["circle"]
		if shapes < 8 {
			t.Fatalf("missing actual diagram geometry: %d", shapes)
		}
	}
}

func TestInputBudgetsAndGridErrorsDoNotProducePartialArtwork(t *testing.T) {
	for _, source := range []string{"", " \n", "a\x00b", "a\tb", strings.Repeat("x", 257), strings.Repeat("x\n", 129), strings.Repeat("x", maxSource+1)} {
		result, err := render(source, false)
		if err == nil || len(result) != 0 {
			t.Fatalf("accepted invalid source of %d bytes", len(source))
		}
	}
}

func TestOutputBudgetRefusesOverflowBeforePublishingIt(t *testing.T) {
	var output svgBuffer
	if _, err := output.Write(bytes.Repeat([]byte{'x'}, maxSVG)); err != nil {
		t.Fatal(err)
	}
	if _, err := output.Write([]byte("overflow")); err == nil {
		t.Fatal("unbounded output")
	}
	if output.data.Len() != maxSVG {
		t.Fatal("overflow must not enter the buffer")
	}
}

func TestMarkersMatchThePinnedGoATCompatibilityFixture(t *testing.T) {
	source, err := os.ReadFile("../../vendor/goat/ascii/examples/line-decorations.txt")
	if err != nil {
		t.Fatal(err)
	}
	result, err := render(string(source), false)
	if err != nil {
		t.Fatal(err)
	}
	count := svgElements(t, result)
	// The related Rust renderer produced only 11 arrow polygons and left six
	// graphical markers as text on this very fixture. These are GoAT 0.5.1's
	// actual defaults, verified against its separately built original CLI.
	if count["polygon"] != 15 || count["circle"] != 13 || count["text"] != 0 {
		t.Fatalf("changed upstream marker interpretation: %v", count)
	}
}

func svgElements(t *testing.T, result []byte) map[string]int {
	t.Helper()
	decoder := xml.NewDecoder(bytes.NewReader(result))
	count := map[string]int{}
	for {
		token, err := decoder.Token()
		if err == io.EOF {
			return count
		}
		if err != nil {
			t.Fatal(err)
		}
		if start, ok := token.(xml.StartElement); ok {
			count[start.Name.Local]++
		}
	}
}
