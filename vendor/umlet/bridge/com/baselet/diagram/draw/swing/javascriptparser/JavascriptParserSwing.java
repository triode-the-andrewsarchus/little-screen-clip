/* Local data-only UMLet adapter. SPDX-License-Identifier: GPL-3.0-only */
package com.baselet.diagram.draw.swing.javascriptparser;
import com.baselet.diagram.draw.DrawHandler;
import com.baselet.diagram.draw.JavascriptCodeParser;
/** Rhino is deliberately absent; source cannot execute JavaScript. */
public final class JavascriptParserSwing extends JavascriptCodeParser {
    public JavascriptParserSwing(DrawHandler drawer) { setDrawer(drawer); }
    @Override public void parse(String code, int width, int height) {
        throw new IllegalArgumentException("Custom JavaScript (_umletcode=) is unsupported");
    }
}
