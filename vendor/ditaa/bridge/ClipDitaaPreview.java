/* Local data-only bridge to Ditaa 0.11.0. SPDX-License-Identifier: MIT OR Apache-2.0 */
import java.io.PrintStream;
import java.nio.ByteBuffer;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Locale;
import org.stathissideris.ascii2image.core.ConversionOptions;
import org.stathissideris.ascii2image.graphics.Diagram;
import org.stathissideris.ascii2image.graphics.SVGRenderer;
import org.stathissideris.ascii2image.text.TextGrid;

/** One bounded UTF-8 stdin canvas in, one SVG stdout artifact out. No paths. */
public final class ClipDitaaPreview {
    private static final int MAX_SOURCE = 256 * 1024;
    private static final int MAX_SVG = 2 * 1024 * 1024;

    public static void main(String[] args) {
        PrintStream output = System.out;
        // Canonical diagnostic output must never become a partial SVG artifact.
        System.setOut(System.err);
        Locale.setDefault(Locale.ROOT);
        try {
            if (args.length != 0) throw new IllegalArgumentException("No arguments are accepted");
            byte[] bytes = System.in.readNBytes(MAX_SOURCE + 1);
            if (bytes.length > MAX_SOURCE) throw new IllegalArgumentException("Source exceeds 256 KiB");
            String source = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(bytes)).toString();
            if (source.startsWith("\ufeff")) source = source.substring(1);
            ConversionOptions options = new ConversionOptions();
            source = canvas(source.replace("\r\n", "\n"), options);
            checkCells(source);
            TextGrid grid = new TextGrid();
            grid.initialiseWithText(source, options.processingOptions);
            Diagram diagram = new Diagram(grid, options);
            String svg = new SVGRenderer().renderToImage(diagram, options.renderingOptions);
            byte[] result = svg.getBytes(StandardCharsets.UTF_8);
            if (result.length > MAX_SVG) throw new IllegalArgumentException("SVG exceeds 2 MiB");
            output.write(result);
            output.flush();
        } catch (Exception | LinkageError error) {
            String message = error.getMessage();
            if (message == null || message.isBlank()) message = error.getClass().getSimpleName();
            System.err.println("Ditaa: " + message.substring(0, Math.min(message.length(), 2048)));
            System.exit(1);
        }
    }

    private static String canvas(String source, ConversionOptions options) {
        String[] lines = source.split("\n", -1);
        int start = 0;
        while (start < lines.length && lines[start].isBlank()) start++;
        if (start == lines.length) throw new IllegalArgumentException("Source is empty");
        if (!lines[start].stripLeading().startsWith("@start")) {
            for (String line : lines) {
                if (line.stripLeading().startsWith("@endditaa")) {
                    throw new IllegalArgumentException("Unexpected @endditaa without @startditaa");
                }
            }
            return source;
        }
        String header = lines[start].trim();
        if (!header.equals("@startditaa") && !header.startsWith("@startditaa ")
                && !header.startsWith("@startditaa\t")) {
            throw new IllegalArgumentException("Expected @startditaa");
        }
        boolean hasScale = false;
        for (String option : header.substring(11).trim().split("[\\s,]+")) {
            if (option.startsWith("scale=")) {
                if (hasScale) throw new IllegalArgumentException("Duplicate Ditaa scale option");
                hasScale = true;
            }
            if (!option.isEmpty()) applyOption(option, options);
        }
        int end = lines.length - 1;
        while (end > start && lines[end].isBlank()) end--;
        if (!lines[end].trim().equals("@endditaa")) {
            throw new IllegalArgumentException("Missing closing @endditaa");
        }
        String[] body = Arrays.copyOfRange(lines, start + 1, end);
        for (String line : body) {
            if (line.stripLeading().startsWith("@start") || line.stripLeading().startsWith("@end")) {
                throw new IllegalArgumentException("Preview accepts one complete @startditaa block");
            }
        }
        return String.join("\n", body);
    }

    private static void applyOption(String option, ConversionOptions options) {
        switch (option) {
            case "-E": case "--no-separation":
                options.processingOptions.setPerformSeparationOfCommonEdges(false); return;
            case "-S": case "--no-shadows":
                options.renderingOptions.setDropShadows(false); return;
            case "-r": case "--round-corners":
                options.processingOptions.setAllCornersAreRound(true); return;
            case "-A": case "--no-antialias":
                options.renderingOptions.setAntialias(false); return;
            case "-T": case "--transparent":
                options.renderingOptions.setBackgroundColor(new java.awt.Color(0, 0, 0, 0)); return;
            case "-W": case "--fixed-slope":
                options.renderingOptions.setFixedSlope(true); return;
            default:
                if (option.startsWith("scale=")) {
                    float scale;
                    try {
                        scale = Float.parseFloat(option.substring(6));
                    } catch (NumberFormatException error) {
                        throw new IllegalArgumentException("Ditaa scale must be a number between 0.25 and 4");
                    }
                    if (!Float.isFinite(scale) || scale < 0.25f || scale > 4.0f) {
                        throw new IllegalArgumentException("Ditaa scale must be between 0.25 and 4");
                    }
                    options.renderingOptions.setScale(scale); return;
                }
                throw new IllegalArgumentException("Unsupported Ditaa preview option: " + option);
        }
    }

    private static void checkCells(String source) {
        if (source.isBlank()) throw new IllegalArgumentException("Source is empty");
        String[] lines = source.split("\n");
        if (lines.length > 128) throw new IllegalArgumentException("Canvas exceeds 128 rows");
        for (String line : lines) {
            int columns = 0;
            for (int index = 0; index < line.length(); index++) {
                char cell = line.charAt(index);
                if (cell == '\t') columns += 8 - columns % 8;
                else {
                    if (Character.isISOControl(cell) || cell == '\ufffe' || cell == '\uffff') {
                        throw new IllegalArgumentException("Unsupported control character in canvas");
                    }
                    columns++;
                }
                if (columns > 256) throw new IllegalArgumentException("Canvas exceeds 256 columns");
            }
        }
    }
}
