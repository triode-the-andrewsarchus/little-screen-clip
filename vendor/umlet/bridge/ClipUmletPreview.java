/* Fixed in-memory UMLet adapter. SPDX-License-Identifier: GPL-3.0-only */
import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.io.StringReader;
import java.nio.ByteBuffer;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import javax.xml.XMLConstants;
import javax.xml.parsers.DocumentBuilderFactory;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.xml.sax.InputSource;
import com.baselet.control.basics.geom.Rectangle;
import com.baselet.control.config.Config;
import com.baselet.control.enums.ElementId;
import com.baselet.control.enums.Program;
import com.baselet.control.enums.RuntimeType;
import com.baselet.diagram.DiagramHandler;
import com.baselet.diagram.io.OutputHandler;
import com.baselet.element.ElementFactorySwing;
import com.baselet.element.interfaces.GridElement;
import com.baselet.gui.command.HelpPanelChanged;

/** Source is only XML data; no upstream CLI, reflection loader or configuration. */
public final class ClipUmletPreview {
    private static final int MAX_SOURCE = 256 * 1024;
    private static final int MAX_SVG = 2 * 1024 * 1024;

    public static void main(String[] args) {
        PrintStream output = System.out;
        System.setOut(System.err);
        java.util.Locale.setDefault(java.util.Locale.ROOT);
        try {
            if (args.length != 0) throw new IllegalArgumentException("No arguments are accepted");
            byte[] bytes = System.in.readNBytes(MAX_SOURCE + 1);
            if (bytes.length > MAX_SOURCE) throw new IllegalArgumentException("Source exceeds 256 KiB");
            String source = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(bytes)).toString();
            if (source.startsWith("\ufeff")) source = source.substring(1);
            if (source.isBlank()) throw new IllegalArgumentException("UMLet source is empty");
            Element root = parse(source);
            if (!root.getTagName().equals("diagram")) throw new IllegalArgumentException("Expected UMLet <diagram> root");
            List<Map<String, Element>> elements = new ArrayList<>();
            collect(root, elements, 0);
            if (elements.isEmpty()) throw new IllegalArgumentException("UMLet diagram has no elements");
            int zoom = 10;
            String help = "";
            for (Map.Entry<String, Element> field : fields(root, true).entrySet()) {
                if (field.getKey().equals("zoom_level")) zoom = integer(field.getValue(), 1, 40);
                if (field.getKey().equals("help_text")) help = field.getValue().getTextContent();
            }
            // Validate all source before constructing a canonical element.
            for (Map<String, Element> element : elements) validate(element);
            Program.init("15.1", RuntimeType.BATCH);
            Config.getInstance().setEnable_custom_elements(false);
            DiagramHandler handler = DiagramHandler.forExport(null);
            handler.setGridSize(zoom);
            Double fontSize = HelpPanelChanged.getFontsize(help);
            if (fontSize != null && (!Double.isFinite(fontSize) || fontSize < 1 || fontSize > 256)) {
                throw new IllegalArgumentException("Diagram fontsize must be between 1 and 256");
            }
            handler.getFontHandler().setDiagramDefaultFontSize(fontSize);
            handler.getFontHandler().setDiagramDefaultFontFamily(HelpPanelChanged.getFontfamily(help));
            for (Map<String, Element> fields : elements) {
                Map<String, Element> coordinates = fields(fields.get("coordinates"), false);
                Rectangle bounds = new Rectangle(integer(coordinates.get("x"), -16384, 16384),
                    integer(coordinates.get("y"), -16384, 16384),
                    integer(coordinates.get("w"), 1, 8192), integer(coordinates.get("h"), 1, 8192));
                String panel = text(fields, "panel_attributes");
                String additional = text(fields, "additional_attributes");
                GridElement element;
                if (fields.containsKey("id")) {
                    element = ElementFactorySwing.create(ElementId.valueOf(text(fields, "id")),
                        bounds, panel, additional, handler);
                } else {
                    element = LegacyFactory.create(text(fields, "type"));
                    element.setRectangle(bounds);
                    element.setPanelAttributes(panel);
                    element.setAdditionalAttributes(additional);
                    handler.setHandlerAndInitListeners(element);
                }
                handler.getDrawPanel().addElement(element);
            }
            ByteArrayOutputStream result = new ByteArrayOutputStream();
            OutputHandler.createToStream("svg", result, handler);
            if (result.size() > MAX_SVG) throw new IllegalArgumentException("SVG exceeds 2 MiB");
            output.write(result.toByteArray());
            output.flush();
            // Swing can start a non-daemon event thread even in headless mode.
            System.exit(0);
        } catch (Exception | LinkageError error) {
            String message = error.getMessage();
            if (message == null || message.isBlank()) message = error.getClass().getSimpleName();
            System.err.println("UMLet: " + message.substring(0, Math.min(2048, message.length())));
            System.exit(1);
        }
    }

    private static Element parse(String source) throws Exception {
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
        factory.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
        factory.setFeature("http://xml.org/sax/features/external-general-entities", false);
        factory.setFeature("http://xml.org/sax/features/external-parameter-entities", false);
        factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
        factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");
        factory.setAttribute("http://www.oracle.com/xml/jaxp/properties/maxElementDepth", "32");
        factory.setXIncludeAware(false);
        factory.setExpandEntityReferences(false);
        return factory.newDocumentBuilder().parse(new InputSource(new StringReader(source))).getDocumentElement();
    }

    private static void collect(Element parent, List<Map<String, Element>> elements, int depth) {
        if (depth > 8) throw new IllegalArgumentException("UMLet group nesting exceeds 8");
        for (Node child = parent.getFirstChild(); child != null; child = child.getNextSibling()) {
            if (!(child instanceof Element)) continue;
            Element element = (Element) child;
            switch (element.getTagName()) {
                case "element":
                    if (elements.size() >= 512) throw new IllegalArgumentException("Diagram exceeds 512 elements");
                    elements.add(fields(element, false)); break;
                case "group": collect(element, elements, depth + 1); break;
                case "help_text": case "zoom_level": break;
                default: throw new IllegalArgumentException("Unsupported UMLet XML field: " + element.getTagName());
            }
        }
    }

    private static Map<String, Element> fields(Element parent, boolean root) {
        Map<String, Element> fields = new LinkedHashMap<>();
        for (Node node = parent.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (!(node instanceof Element)) continue;
            Element child = (Element) node;
            String name = child.getTagName();
            if (root && (name.equals("element") || name.equals("group"))) continue;
            if (name.equals("custom_code")) throw new IllegalArgumentException("Custom Java elements are unsupported");
            if (fields.put(name, child) != null) throw new IllegalArgumentException("Duplicate UMLet field: " + name);
        }
        return fields;
    }

    private static String text(Map<String, Element> fields, String key) {
        Element field = fields.get(key);
        if (field == null) return "";
        for (Node node = field.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (node instanceof Element) throw new IllegalArgumentException("Nested XML in UMLet text field: " + key);
        }
        return field.getTextContent();
    }

    private static int integer(Element field, int minimum, int maximum) {
        if (field == null) throw new IllegalArgumentException("Missing UMLet coordinate/zoom value");
        for (Node node = field.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (node instanceof Element) throw new IllegalArgumentException("Nested XML in UMLet coordinate/zoom");
        }
        int value = Integer.parseInt(field.getTextContent().trim());
        if (value < minimum || value > maximum) throw new IllegalArgumentException("UMLet coordinate/zoom exceeds preview limits");
        return value;
    }

    private static void validate(Map<String, Element> element) {
        for (String key : element.keySet()) {
            if (!List.of("id", "type", "coordinates", "panel_attributes", "additional_attributes").contains(key)) {
                throw new IllegalArgumentException("Unsupported UMLet element field: " + key);
            }
        }
        if (element.containsKey("id") == element.containsKey("type")) {
            throw new IllegalArgumentException("An element requires exactly one UMLet id or legacy type");
        }
        if (element.containsKey("id")) {
            try { ElementId.valueOf(text(element, "id")); }
            catch (IllegalArgumentException error) {
                throw new IllegalArgumentException("Unsupported UMLet element id: " + text(element, "id"));
            }
        } else LegacyFactory.validate(text(element, "type"));
        if (!element.containsKey("coordinates")) throw new IllegalArgumentException("Missing UMLet coordinates");
        Map<String, Element> coordinates = fields(element.get("coordinates"), false);
        if (!coordinates.keySet().equals(java.util.Set.of("x", "y", "w", "h"))) {
            throw new IllegalArgumentException("UMLet coordinates require x, y, w and h");
        }
        integer(coordinates.get("x"), -16384, 16384);
        integer(coordinates.get("y"), -16384, 16384);
        integer(coordinates.get("w"), 1, 8192);
        integer(coordinates.get("h"), 1, 8192);
        String panel = text(element, "panel_attributes");
        for (String line : panel.split("\n")) {
            if (line.trim().startsWith("_umletcode=")) {
                throw new IllegalArgumentException("Custom JavaScript (_umletcode=) is unsupported");
            }
        }
        text(element, "additional_attributes");
    }
}
