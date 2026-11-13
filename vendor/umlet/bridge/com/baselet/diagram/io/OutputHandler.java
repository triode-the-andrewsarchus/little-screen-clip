/* SVG-only adaptation of UMLet 15.1 OutputHandler (GPL-3.0).
 * Original canonical SVG/paint routines retained; see ADAPTATIONS.md.
 */
package com.baselet.diagram.io;
import java.awt.Component;
import java.awt.Color;
import java.awt.Graphics2D;
import java.io.IOException;
import java.io.OutputStream;
import java.util.Collection;
import javax.swing.JLayeredPane;
import org.apache.batik.dom.GenericDOMImplementation;
import org.apache.batik.svggen.SVGGraphics2D;
import org.w3c.dom.DOMImplementation;
import org.w3c.dom.Element;
import com.baselet.control.basics.Converter;
import com.baselet.control.basics.geom.Rectangle;
import com.baselet.control.config.Config;
import com.baselet.control.constants.Constants;
import com.baselet.diagram.DiagramHandler;
import com.baselet.diagram.DrawPanel;
import com.baselet.diagram.FontHandler;
import com.baselet.element.ElementFactorySwing;
import com.baselet.element.interfaces.GridElement;

public final class OutputHandler {
	public static void createToStream(String extension, OutputStream ostream, DiagramHandler handler) throws Exception {

		int oldZoom = handler.getGridSize();
		handler.setGridAndZoom(Constants.DEFAULTGRIDSIZE, false); // Zoom to the defaultGridsize before execution

		// if some GridElements are selected, only export them
		Collection<GridElement> elementsToDraw = handler.getDrawPanel().getSelector().getSelectedElements();
		// if nothing is selected, draw everything
		if (elementsToDraw.isEmpty()) {
			elementsToDraw = handler.getDrawPanel().getGridElements();
		}

		if (!"svg".equals(extension)) throw new IllegalArgumentException("Only SVG export is available");
        for (GridElement element : elementsToDraw) element.getDeprecatedAddons().doBeforeExport();
        exportSvg(ostream, elementsToDraw, handler.getFontHandler());

		handler.setGridAndZoom(oldZoom, false); // Zoom back to the oldGridsize after execution
	}

	private static void exportSvg(OutputStream ostream, Collection<GridElement> entities, FontHandler diagramFont) throws IOException {
		Rectangle bounds = DrawPanel.getContentBounds(Config.getInstance().getPrintPadding(), entities);
		DOMImplementation domImpl = GenericDOMImplementation.getDOMImplementation();
		org.w3c.dom.Document document = domImpl.createDocument(null, "svg", null);

		SVGGraphics2D graphics2d = new SVGGraphics2D(document);
		graphics2d.setSVGCanvasSize(Converter.convert(bounds.getSize()));
		paintEntitiesIntoGraphics2D(graphics2d, entities, diagramFont);

		Element root = graphics2d.getRoot();
		root.setAttributeNS(null, "viewBox", String.format("%d %d %d %d", bounds.x, bounds.y, bounds.width, bounds.height));
		// Batik's stream method inserts a public SVG DOCTYPE. Serialize this
		// generated geometry DOM directly: no DTD is needed or accepted by the
		// application's shared SVG parser. Authored XML never enters this DOM.
		try {
			javax.xml.transform.TransformerFactory factory = javax.xml.transform.TransformerFactory.newInstance();
			factory.setFeature(javax.xml.XMLConstants.FEATURE_SECURE_PROCESSING, true);
			factory.setAttribute(javax.xml.XMLConstants.ACCESS_EXTERNAL_DTD, "");
			factory.setAttribute(javax.xml.XMLConstants.ACCESS_EXTERNAL_STYLESHEET, "");
			factory.newTransformer().transform(new javax.xml.transform.dom.DOMSource(root),
				new javax.xml.transform.stream.StreamResult(ostream));
		} catch (javax.xml.transform.TransformerException error) {
			throw new IOException("Could not serialize local UMLet SVG", error);
		}
		graphics2d.dispose();
	}

	public static void paintEntitiesIntoGraphics2D(Graphics2D g2d, Collection<GridElement> entities, FontHandler diagramFont) {
		DiagramHandler handler = DiagramHandler.forExport(diagramFont); // #290: pass fontHandler from original diagramHandler to let the export use diagram specific fontsize+family
		JLayeredPane tempPanel = new JLayeredPane();
		for (GridElement entity : entities) {
			GridElement clone = ElementFactorySwing.createCopy(entity, handler);
			com.baselet.element.interfaces.Component component = clone.getComponent();
			// Issue 138: when PDF and Swing Export draw on (0,0) a part of the drawn image is cut, therefore it's displaced by 0.5px in that case.
			// also Issue 270: makes arrow ending placement better
			component.translateForExport();
			tempPanel.add((Component) component, clone.getLayer());
		}
		tempPanel.validate();
		tempPanel.setBackground(Color.WHITE);
		tempPanel.setSize(Integer.MAX_VALUE, Integer.MAX_VALUE);
		tempPanel.update(g2d);
	}
}
