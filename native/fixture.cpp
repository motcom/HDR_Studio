#include <Alembic/AbcGeom/All.h>
#include <Alembic/AbcCoreOgawa/All.h>
#include <iostream>
namespace A = Alembic::Abc;
namespace G = Alembic::AbcGeom;
int main(int argc, char** argv) {
    if (argc != 2) return 2;
    try {
        A::OArchive archive(Alembic::AbcCoreOgawa::WriteArchive(), argv[1]);
        auto top = archive.getTop();
        G::OXform root(top, "asset");
        G::XformSample transform;
        transform.setTranslation(A::V3d(2, 0, 0));
        root.getSchema().set(transform);
        G::OXform child(root, "model");
        G::XformSample scale;
        scale.setScale(A::V3d(1, 1.5, 1));
        child.getSchema().set(scale);
        G::OPolyMesh mesh(child, "cubeShape");
        std::vector<A::V3f> p = {{-1,-1,-1},{1,-1,-1},{1,1,-1},{-1,1,-1},
                                {-1,-1,1},{1,-1,1},{1,1,1},{-1,1,1}};
        std::vector<int32_t> i = {0,3,2,1,4,5,6,7,0,1,5,4,3,7,6,2,0,4,7,3,1,2,6,5};
        std::vector<int32_t> c(6, 4);
        mesh.getSchema().set(G::OPolyMeshSchema::Sample(A::P3fArraySample(p), A::Int32ArraySample(i), A::Int32ArraySample(c)));
        G::OXform camera_x(top, "shotCamera");
        G::XformSample camera_transform;
        camera_transform.setTranslation(A::V3d(2, 0, 8));
        camera_x.getSchema().set(camera_transform);
        G::OCamera camera(camera_x, "shotCameraShape");
        G::CameraSample s;
        s.setFocalLength(50); s.setHorizontalAperture(3.6); s.setVerticalAperture(2.025);
        s.setHorizontalFilmOffset(0.18); s.setNearClippingPlane(0.1); s.setFarClippingPlane(1000);
        camera.getSchema().set(s);
        std::cout << "Wrote hierarchy, nonuniform scale, quad cube, offset 50mm camera\n";
    } catch (const std::exception& e) { std::cerr << e.what() << '\n'; return 1; }
}
