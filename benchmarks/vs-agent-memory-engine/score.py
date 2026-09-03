#!/usr/bin/env python3
import json
from pathlib import Path
D = Path(__file__).resolve().parent

# ground-truth declaring file(s) per term, established by manual grep earlier
DECL = {
  ("golang-gin-realworld-example-app","ArticleModel"): ["articles/models.go"],
  ("golang-gin-realworld-example-app","UserModel"): ["users/models.go"],
  ("golang-gin-realworld-example-app","CommentModel"): ["articles/models.go"],
  ("golang-gin-realworld-example-app","TagModel"): ["articles/models.go"],
  ("golang-gin-realworld-example-app","Follow"): ["users/models.go","users/routers.go","users/serializers.go"],
  ("nestjs-realworld-example-app","ArticleEntity"): ["src/article/article.entity.ts"],
  ("nestjs-realworld-example-app","UserEntity"): ["src/user/user.entity.ts"],
  ("nestjs-realworld-example-app","TagEntity"): ["src/tag/tag.entity.ts"],
  ("nestjs-realworld-example-app","FollowsEntity"): ["src/profile/follows.entity.ts"],
  ("nestjs-realworld-example-app","Comment"): ["src/article/comment.entity.ts"],
  ("spring-petclinic","Owner"): ["src/main/java/org/springframework/samples/petclinic/owner/Owner.java"],
  ("spring-petclinic","Pet"): ["src/main/java/org/springframework/samples/petclinic/owner/Pet.java"],
  ("spring-petclinic","Vet"): ["src/main/java/org/springframework/samples/petclinic/vet/Vet.java"],
  ("spring-petclinic","Visit"): ["src/main/java/org/springframework/samples/petclinic/owner/Visit.java"],
  ("spring-petclinic","PetType"): ["src/main/java/org/springframework/samples/petclinic/owner/PetType.java"],
}

total_exist = 0
arch_correct_hit = 0
ame_hit = 0
for f in sorted(D.glob("result_*.json")):
    repo = f.stem.replace("result_", "")
    data = json.loads(f.read_text())
    for e in data:
        term = e["term"]
        key = (repo, term)
        if key not in DECL:
            continue
        total_exist += 1
        decls = DECL[key]
        # archietect: check evidence 'what' strings contain any decl file
        ev = e["archietect"]["result"].get("evidence", [])
        arch_files = " ".join(x.get("what","") for x in ev)
        arch_hit = any(d in arch_files for d in decls)
        arch_correct_hit += arch_hit

        ar = e["ame_architecture_review"]
        ame_paths = [t["path"] for t in ar.get("trace_paths", [])]
        this_hit = any(any(d == p for d in decls) for p in ame_paths)
        ame_hit += this_hit
        print(f"{repo:35s} {term:15s} archietect_hit={arch_hit} ame_hit={this_hit}  ame_top_paths={ame_paths[:4]}")

print(f"\nTOTALS over N={total_exist} EXISTS queries with a known single/near declaration file:")
print(f"  archietect surfaced declaring file: {arch_correct_hit}/{total_exist}")
print(f"  AME surfaced declaring file in top-10 trace: {ame_hit}/{total_exist}")
