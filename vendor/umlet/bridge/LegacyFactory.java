/* Fixed builtin constructor table. SPDX-License-Identifier: GPL-3.0-only */
import java.util.Map;
import java.util.function.Supplier;
import com.baselet.element.interfaces.GridElement;
/** Authored class names never enter reflection or a compiler. */
final class LegacyFactory {
    private static final Map<String, Supplier<GridElement>> BUILTINS = Map.ofEntries(
        Map.entry("ActiveClass", com.baselet.element.old.element.ActiveClass::new),
        Map.entry("Actor", com.baselet.element.old.element.Actor::new),
        Map.entry("AlternativeUseCase", com.baselet.element.old.element.AlternativeUseCase::new),
        Map.entry("Artefact", com.baselet.element.old.element.Artefact::new),
        Map.entry("Class", com.baselet.element.old.element.Class::new),
        Map.entry("Component", com.baselet.element.old.element.Component::new),
        Map.entry("Database", com.baselet.element.old.element.Database::new),
        Map.entry("Decision", com.baselet.element.old.element.Decision::new),
        Map.entry("EER_Entity", com.baselet.element.old.element.EER_Entity::new),
        Map.entry("EER_Rel_Diamond", com.baselet.element.old.element.EER_Rel_Diamond::new),
        Map.entry("FinalState", com.baselet.element.old.element.FinalState::new),
        Map.entry("HistoryState", com.baselet.element.old.element.HistoryState::new),
        Map.entry("InitialFinalState", com.baselet.element.old.element.InitialFinalState::new),
        Map.entry("InitialState", com.baselet.element.old.element.InitialState::new),
        Map.entry("InteractionFrame", com.baselet.element.old.element.InteractionFrame::new),
        Map.entry("Interface", com.baselet.element.old.element.Interface::new),
        Map.entry("Node", com.baselet.element.old.element.Node::new),
        Map.entry("Note", com.baselet.element.old.element.Note::new),
        Map.entry("Package", com.baselet.element.old.element.Package::new),
        Map.entry("PackageAggregationHierarchy", com.baselet.element.old.element.PackageAggregationHierarchy::new),
        Map.entry("ReceiveSignal", com.baselet.element.old.element.ReceiveSignal::new),
        Map.entry("RegionEndState", com.baselet.element.old.element.RegionEndState::new),
        Map.entry("Relation", com.baselet.element.old.element.Relation::new),
        Map.entry("SendSignal", com.baselet.element.old.element.SendSignal::new),
        Map.entry("SeqDestroyMark", com.baselet.element.old.element.SeqDestroyMark::new),
        Map.entry("SeqObjectActive", com.baselet.element.old.element.SeqObjectActive::new),
        Map.entry("SeqSelfMessage", com.baselet.element.old.element.SeqSelfMessage::new),
        Map.entry("Signal", com.baselet.element.old.element.Signal::new),
        Map.entry("SimpleComponent", com.baselet.element.old.element.SimpleComponent::new),
        Map.entry("Socket", com.baselet.element.old.element.Socket::new),
        Map.entry("State", com.baselet.element.old.element.State::new),
        Map.entry("StateSubDiagram", com.baselet.element.old.element.StateSubDiagram::new),
        Map.entry("SynchBar", com.baselet.element.old.element.SynchBar::new),
        Map.entry("SynchBarHorizontal", com.baselet.element.old.element.SynchBarHorizontal::new),
        Map.entry("SynchBarVertical", com.baselet.element.old.element.SynchBarVertical::new),
        Map.entry("Systemborder", com.baselet.element.old.element.Systemborder::new),
        Map.entry("Taxonomy_of_Actors", com.baselet.element.old.element.Taxonomy_of_Actors::new),
        Map.entry("Taxonomy_of_Workprocesses", com.baselet.element.old.element.Taxonomy_of_Workprocesses::new),
        Map.entry("TemplateClass", com.baselet.element.old.element.TemplateClass::new),
        Map.entry("Text", com.baselet.element.old.element.Text::new),
        Map.entry("ThreeWayRelation", com.baselet.element.old.element.ThreeWayRelation::new),
        Map.entry("TimeSignal", com.baselet.element.old.element.TimeSignal::new),
        Map.entry("UseCase", com.baselet.element.old.element.UseCase::new),
        Map.entry("ActivityDiagramText", com.baselet.element.old.allinone.ActivityDiagramText::new),
        Map.entry("SequenceDiagram", com.baselet.element.old.allinone.SequenceDiagram::new));
    static void validate(String type) {
        if (!type.matches("(?:com\\.umlet\\.element(?:\\.base|\\.allinone)?|com\\.baselet\\.element\\.old\\.(?:element|allinone))\\.[A-Za-z_]+")) {
            throw new IllegalArgumentException("Unsupported legacy UMLet type: " + type);
        }
        String name = type.substring(type.lastIndexOf('.') + 1);
        Supplier<GridElement> constructor = BUILTINS.get(name);
        if (constructor == null) throw new IllegalArgumentException("Unsupported legacy UMLet type: " + type);
    }
    static GridElement create(String type) {
        validate(type);
        return BUILTINS.get(type.substring(type.lastIndexOf('.') + 1)).get();
    }
}
