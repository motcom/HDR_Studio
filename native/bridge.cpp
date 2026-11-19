#include "bridge.h"
#include <Alembic/AbcGeom/All.h>
#include <Alembic/AbcCoreOgawa/All.h>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <memory>
#include <stdexcept>
#include <vector>

namespace A = Alembic::Abc;
namespace G = Alembic::AbcGeom;
struct Node {
    AbcNode info{};
    std::string path;
    std::vector<float> positions, normals;
    std::vector<int32_t> indices, counts;
};
struct Scene { std::vector<Node> nodes; };

static void visit(Scene& scene, A::IObject object, int32_t parent,
                  const A::M44d& parent_world, bool parent_visible,
                  const A::ISampleSelector& sample) {
    Node n;
    n.path = object.getFullName();
    n.info.parent = parent;
    A::M44d world = parent_world;
    const auto& header = object.getHeader();
    if (G::IXform::matches(header)) {
        n.info.kind = 1;
        G::XformSample x;
        G::IXform(object, A::kWrapExisting).getSchema().get(x, sample);
        world = x.getInheritsXforms() ? x.getMatrix() * parent_world : x.getMatrix();
    } else if (G::IPolyMesh::matches(header)) {
        n.info.kind = 2;
        auto schema = G::IPolyMesh(object, A::kWrapExisting).getSchema();
        G::IPolyMeshSchema::Sample mesh;
        schema.get(mesh, sample);
        auto positions = mesh.getPositions();
        auto indices = mesh.getFaceIndices();
        auto counts = mesh.getFaceCounts();
        if (!positions || !indices || !counts) throw std::runtime_error("Incomplete mesh: " + n.path);
        for (size_t pi = 0; pi < positions->size(); ++pi) {
            auto p = (*positions)[pi];
            if (!std::isfinite(p.x) || !std::isfinite(p.y) || !std::isfinite(p.z))
                throw std::runtime_error("Non-finite position: " + n.path);
            n.positions.insert(n.positions.end(), {p.x, p.y, p.z});
        }
        n.indices.assign(indices->get(), indices->get() + indices->size());
        n.counts.assign(counts->get(), counts->get() + counts->size());
        uint64_t corners = 0;
        for (auto count : n.counts) {
            if (count < 3) throw std::runtime_error("Face has fewer than 3 corners: " + n.path);
            corners += count;
        }
        if (corners != n.indices.size()) throw std::runtime_error("Invalid face counts: " + n.path);
        for (auto i : n.indices) if (i < 0 || size_t(i) >= positions->size())
            throw std::runtime_error("Invalid vertex index: " + n.path);
        auto param = schema.getNormalsParam();
        if (param.valid()) {
            auto values = param.getExpandedValue(sample).getVals();
            if (values) {
                for (size_t corner = 0; corner < n.indices.size(); ++corner) {
                    size_t index = corner;
                    switch (param.getScope()) {
                    case G::kVertexScope: case G::kVaryingScope: index = n.indices[corner]; break;
                    case G::kConstantScope: index = 0; break;
                    case G::kFacevaryingScope: break;
                    default: index = values->size(); break;
                    }
                    if (index >= values->size()) { n.normals.clear(); break; }
                    auto v = (*values)[index];
                    n.normals.insert(n.normals.end(), {v.x, v.y, v.z});
                }
            }
        }
    } else if (G::ICamera::matches(header)) {
        n.info.kind = 3;
        G::CameraSample camera;
        G::ICamera(object, A::kWrapExisting).getSchema().get(camera, sample);
        for (size_t i = 0; i < 16; ++i) n.info.camera_core[i] = camera.getCoreValue(i);
        auto film = camera.getFilmBackMatrix();
        for (int r = 0; r < 3; ++r) for (int c = 0; c < 3; ++c)
            n.info.film_back[r * 3 + c] = film[r][c];
    } else { n.info.kind = 4; }
    bool visible = parent_visible;
    auto visibility = G::GetVisibilityProperty(object);
    if (visibility.valid()) {
        auto v = visibility.getValue(sample);
        if (v == G::kVisibilityHidden) visible = false;
    }
    n.info.visible = visible ? 1 : 0;
    for (int r = 0; r < 4; ++r) for (int c = 0; c < 4; ++c)
        n.info.world[r * 4 + c] = world[r][c];
    auto id = static_cast<int32_t>(scene.nodes.size());
    scene.nodes.push_back(std::move(n));
    for (size_t i = 0; i < object.getNumChildren(); ++i)
        visit(scene, object.getChild(i), id, world, visible, sample);
}

uint32_t abc_abi_version() { return 1; }
void* abc_open(const char* path, double seconds, char* error, uint64_t capacity) {
    try {
        if (!path || !std::isfinite(seconds)) throw std::runtime_error("Invalid path/time");
        // filesystem::path opens Unicode Windows paths without relying on the ANSI code page.
        std::ifstream stream(std::filesystem::u8path(path), std::ios::binary);
        if (!stream) throw std::runtime_error("Cannot open Alembic file");
        Alembic::AbcCoreOgawa::ReadArchive reader(std::vector<std::istream*>{&stream});
        A::IArchive archive(reader, "stream.abc", A::ErrorHandler::kThrowPolicy);
        if (!archive.valid()) throw std::runtime_error("Not a supported Alembic archive (Ogawa required)");
        auto scene = std::make_unique<Scene>();
        A::M44d identity; identity.makeIdentity();
        auto top = archive.getTop();
        A::ISampleSelector sample(seconds, A::ISampleSelector::kNearIndex);
        for (size_t i = 0; i < top.getNumChildren(); ++i)
            visit(*scene, top.getChild(i), -1, identity, true, sample);
        return scene.release();
    } catch (const std::exception& e) {
        if (error && capacity) {
            auto count = std::min<uint64_t>(std::strlen(e.what()), capacity - 1);
            std::memcpy(error, e.what(), size_t(count)); error[count] = 0;
        }
    } catch (...) {
        if (error && capacity) { const char* msg = "Unknown Alembic error";
            auto count = std::min<uint64_t>(std::strlen(msg), capacity - 1);
            std::memcpy(error, msg, size_t(count)); error[count] = 0;
        }
    }
    return nullptr;
}
uint64_t abc_node_count(const void* scene) { return scene ? static_cast<const Scene*>(scene)->nodes.size() : 0; }
int32_t abc_get_node(const void* scene, uint64_t index, AbcNode* out) {
    if (!scene || !out) return 0;
    const auto& nodes = static_cast<const Scene*>(scene)->nodes;
    if (index >= nodes.size()) return 0;
    const auto& n = nodes[size_t(index)];
    *out = n.info;
    out->path = n.path.c_str();
    out->positions = n.positions.data(); out->position_count = n.positions.size() / 3;
    out->indices = n.indices.data(); out->index_count = n.indices.size();
    out->face_counts = n.counts.data(); out->face_count = n.counts.size();
    out->corner_normals = n.normals.data(); out->normal_count = n.normals.size() / 3;
    return 1;
}
void abc_close(void* scene) { delete static_cast<Scene*>(scene); }
