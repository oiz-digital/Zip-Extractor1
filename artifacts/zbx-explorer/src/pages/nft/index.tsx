import React from "react";
import { useGetNftCollections, useGetGamingProjects } from "@workspace/api-client-react";
import { formatNumber, formatCurrency, formatAddress } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Badge } from "@/components/ui/badge";
import { Gamepad2, Image as ImageIcon, CheckCircle2 } from "lucide-react";

export default function NFT() {
  const { data: collections, isLoading: nftsLoading } = useGetNftCollections();
  const { data: games, isLoading: gamesLoading } = useGetGamingProjects();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
          <Gamepad2 className="text-primary w-6 h-6" /> NFT & Gaming
        </h2>
        <p className="text-muted-foreground">Digital assets and interactive entertainment on Zebvix.</p>
      </div>

      <Tabs defaultValue="nft" className="w-full">
        <TabsList className="grid w-full md:w-auto grid-cols-2">
          <TabsTrigger value="nft" className="flex items-center gap-2"><ImageIcon className="w-4 h-4"/> NFT Collections</TabsTrigger>
          <TabsTrigger value="gaming" className="flex items-center gap-2"><Gamepad2 className="w-4 h-4"/> Gaming Projects</TabsTrigger>
        </TabsList>
        
        <TabsContent value="nft" className="mt-4">
          <Card>
            <CardContent className="p-0">
              {nftsLoading ? (
                <div className="p-4 space-y-2">
                  {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-16 w-full" />)}
                </div>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Collection</TableHead>
                      <TableHead className="text-right">Floor Price</TableHead>
                      <TableHead className="text-right">24h Volume</TableHead>
                      <TableHead className="text-right">Supply</TableHead>
                      <TableHead className="text-right">Owners</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {collections?.map((col) => (
                      <TableRow key={col.id}>
                        <TableCell>
                          <div className="flex items-center gap-2">
                            <div className="w-10 h-10 bg-muted rounded-md border border-border flex items-center justify-center font-bold text-muted-foreground text-xs overflow-hidden">
                              {col.symbol}
                            </div>
                            <div className="flex flex-col">
                              <span className="font-bold flex items-center gap-1">
                                {col.name} 
                                {col.verified && <CheckCircle2 className="w-3 h-3 text-primary" />}
                              </span>
                              {col.contractAddress && (
                                <span className="text-[10px] font-mono text-muted-foreground">
                                  {formatAddress(col.contractAddress, 6)}
                                </span>
                              )}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="text-right font-mono font-medium">{formatNumber(col.floorPrice, 2)} ZBX</TableCell>
                        <TableCell className="text-right font-mono">{formatNumber(col.volume24h, 2)} ZBX</TableCell>
                        <TableCell className="text-right font-mono">{formatNumber(col.totalSupply, 0)}</TableCell>
                        <TableCell className="text-right font-mono">{formatNumber(col.owners, 0)}</TableCell>
                      </TableRow>
                    ))}
                    {(!collections || collections.length === 0) && (
                      <TableRow>
                        <TableCell colSpan={5} className="text-center py-12 text-muted-foreground">
                          No NFT collections found.
                        </TableCell>
                      </TableRow>
                    )}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="gaming" className="mt-4">
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
            {gamesLoading ? (
              Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-48 w-full" />)
            ) : (
              games?.map((game) => (
                <Card key={game.id} className="overflow-hidden flex flex-col">
                  <div className="h-2 bg-primary w-full" />
                  <CardContent className="p-6 flex-1 flex flex-col">
                    <div className="flex justify-between items-start mb-4">
                      <div>
                        <h3 className="font-bold text-xl">{game.name}</h3>
                        <Badge variant="secondary" className="mt-1 text-[10px] uppercase">{game.category}</Badge>
                      </div>
                      {game.onChain && <Badge className="bg-green-500/20 text-green-500 border-none hover:bg-green-500/30">Fully On-Chain</Badge>}
                    </div>
                    
                    <p className="text-sm text-muted-foreground mb-6 flex-1">{game.description}</p>
                    
                    <div className="grid grid-cols-3 gap-4 border-t border-border pt-4 mt-auto">
                      <div>
                        <span className="text-xs text-muted-foreground block mb-1">Players (24h)</span>
                        <span className="font-mono font-bold text-lg">{formatNumber(game.players, 0)}</span>
                      </div>
                      <div>
                        <span className="text-xs text-muted-foreground block mb-1">Tx Count (24h)</span>
                        <span className="font-mono font-bold text-lg">{formatNumber(game.txLast24h, 0)}</span>
                      </div>
                      <div>
                        <span className="text-xs text-muted-foreground block mb-1">Total Rev</span>
                        <span className="font-mono font-bold text-lg text-primary">{formatCurrency(game.totalRevenue, 0)}</span>
                      </div>
                    </div>
                  </CardContent>
                </Card>
              ))
            )}
            {!gamesLoading && (!games || games.length === 0) && (
              <div className="col-span-full text-center py-12 text-muted-foreground border border-dashed border-border rounded-lg">
                No gaming projects found.
              </div>
            )}
          </div>
        </TabsContent>
      </Tabs>
    </div>
  );
}
